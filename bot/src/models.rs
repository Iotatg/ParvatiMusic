// models — Track/queue data types.

pub const PLATFORM_YOUTUBE: &str = "YouTube";
pub const PLATFORM_SPOTIFY: &str = "Spotify";
pub const PLATFORM_DEEZER: &str = "Deezer";
pub const PLATFORM_IOTA_API: &str = "IotaApi";
pub const PLATFORM_TELEGRAM: &str = "Telegram";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub duration: i64,
    pub artwork: String,
    pub url: String,
    pub requester: String,
    pub video: bool,
    pub source: String,
    pub is_autoplay: bool,
    pub engine_token: String,
}

impl Track {
    pub fn mark_autoplay(&mut self, who: &str, token: &str) -> &mut Self {
        self.is_autoplay = true;
        self.requester = who.to_string();
        self.engine_token = token.to_string();
        self
    }

 /// autoplay + optional token equality check.
    pub fn is_verified_autoplay(&self, token: &str) -> bool {
        if !self.is_autoplay {
            return false;
        }
        if !token.is_empty() && self.engine_token != token {
            return false;
        }
        true
    }
}

/// lowercase, strip bracketed groups, strip filler words.
pub fn normalize_track_title(title: &str) -> String {
    if title.is_empty() {
        return String::new();
    }
    let t = title.to_lowercase();
    let stripped = strip_bracketed(&t);
    const REPS: [&str; 17] = [
        "official music video",
        "official video",
        "music video",
        "lyric video",
        "lyrics",
        "full video",
        "video song",
        "audio song",
        "audio track",
        "official audio",
        "audio",
        "4k",
        "8k",
        "hd",
        "remix",
        "song",
        "full track",
    ];
    let mut out = stripped;
    for r in REPS {
        while let Some(p) = out.find(r) {
            out.replace_range(p..p + r.len(), " ");
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_bracketed(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}
