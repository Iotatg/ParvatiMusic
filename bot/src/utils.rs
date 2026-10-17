// utils (pure std, no deps).

pub const SMALL_CAPS: [&str; 26] = [
    "ᴀ", "ʙ", "ᴄ", "ᴅ", "є", "ғ", "ɢ", "ʜ", "ɪ", "ᴊ", "ᴋ", "ʟ", "ϻ", "η", "σ", "ᴘ", "ǫ", "ʀ", "s",
    "ᴛ", "υ", "ᴠ", "ᴡ", "x", "ʏ", "ᴢ",
];

pub fn format_duration(sec: i64) -> String {
    let sec = sec.max(0);
    if sec < 60 {
        format!("{sec}s")
    } else if sec < 3600 {
        format!("{}m {}s", sec / 60, sec % 60)
    } else if sec < 86400 {
        format!("{}h {}m", sec / 3600, (sec % 3600) / 60)
    } else {
        format!("{}d {}h", sec / 86400, (sec % 86400) / 3600)
    }
}

fn pad_zero(v: i64) -> String {
    if v < 10 {
        format!("0{v}")
    } else {
        v.to_string()
    }
}

/// (h > 0 -> h:mm:ss, zero-padded).
pub fn format_time(sec: i64) -> String {
    let sec = sec.max(0);
    let (h, m, s) = (sec / 3600, (sec % 3600) / 60, sec % 60);
    if h > 0 {
        format!("{h}:{}:{}", pad_zero(m), pad_zero(s))
    } else {
        format!("{}:{}", pad_zero(m), pad_zero(s))
    }
}

/// (keeps nonzero leading parts, always prints seconds).
pub fn format_uptime(seconds: i64) -> String {
    let mut r = String::new();
    let mut s = seconds.max(0);
    let days = s / 86400;
    s %= 86400;
    let hours = s / 3600;
    s %= 3600;
    let minutes = s / 60;
    let secs = s % 60;
    if days > 0 {
        r.push_str(&format!("{days}d "));
    }
    if hours > 0 {
        r.push_str(&format!("{hours}h "));
    }
    if minutes > 0 {
        r.push_str(&format!("{minutes}m "));
    }
    r.push_str(&format!("{secs}s"));
    r
}

/// 9 fixed bars; index = played*9/duration; ◉ marks position
pub fn get_progress_bar(played_sec: i64, duration_sec: i64) -> String {
    let table: [&str; 9] = [
        "◉———————",
        "—◉——————",
        "——◉—————",
        "———◉————",
        "————◉———",
        "—————◉——",
        "——————◉—",
        "———————◉",
        "————————◉",
    ];
    if duration_sec <= 0 || played_sec <= 0 {
        return table[0].to_string();
    }
    if played_sec >= duration_sec {
        return table[8].to_string();
    }
    let mut index = (played_sec * 9) / duration_sec;
    if index > 8 {
        index = 8;
    }
    table[index as usize].to_string()
}

/// (byte-based truncation, U+2026 suffix).
pub fn short_title(title: &str, max: usize) -> String {
    if title.len() <= max {
        return title.to_string();
    }
    let cut = &title.as_bytes()[..max];
    let mut out = String::from_utf8_lossy(cut).into_owned();
    out.push('\u{2026}');
    out
}

pub fn escape_html(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn trim(s: &str) -> &str {
    s.trim()
}

pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

/// (html_mention/mention).
pub fn html_mention(name: &str, id: i64) -> String {
    let n = trim(name);
    let n = if n.is_empty() { "user" } else { n };
    format!(
        "<a href=\"tg://user?id={id}\">{}</a>",
        escape_html(n)
    )
}

///+ (stylish): smallcaps for ASCII letters, tags/entities untouched.
pub fn stylish(input: &str) -> String {
    if input.is_empty() {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(ch);
            }
            '>' => {
                in_tag = false;
                out.push(ch);
            }
            c if !in_tag && c.is_ascii_lowercase() => {
                out.push_str(SMALL_CAPS[(c as u8 - b'a') as usize]);
            }
            c => out.push(c),
        }
    }
    out
}

/// (human_size: binary units, one decimal).
pub fn human_size(bytes: i64) -> String {
    const UNITS: [char; 6] = ['K', 'M', 'G', 'T', 'P', 'E'];
    let size = bytes as f64;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut exp = 0usize;
    let mut n = size / 1024.0;
    while n >= 1024.0 && exp + 1 < UNITS.len() {
        n /= 1024.0;
        exp += 1;
    }
    format!("{:.1} {}iB", n, UNITS[exp])
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// (bit accumulator, '=' padded to multiple of 4).
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut val: u32 = 0;
    let mut bits: i32 = -6;
    for &c in data {
        val = (val << 8) + c as u32;
        bits += 8;
        while bits >= 0 {
            out.push(B64[((val >> bits) & 0x3F) as usize] as char);
            bits -= 6;
        }
    }
    if bits > -6 {
        out.push(B64[(((val << 8) >> (bits + 8)) & 0x3F) as usize] as char);
    }
    while out.len() % 4 != 0 {
        out.push('=');
    }
    out
}

/// parity: bit accumulator; stops at first '=', skips invalid chars.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut rev = [None; 256];
    for (i, b) in B64.iter().enumerate() {
        rev[*b as usize] = Some(i as u8);
    }
    let mut out: Vec<u8> = Vec::with_capacity(s.len() * 3 / 4);
    let mut val: u32 = 0;
    let mut bits: i32 = -8;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = rev[c as usize] else {
            continue;
        };
        val = (val << 6) + v as u32;
        bits += 6;
        if bits >= 0 {
            out.push(((val >> bits) & 0xFF) as u8);
            bits -= 8;
        }
    }
    Some(out)
}
