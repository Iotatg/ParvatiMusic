use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    pub token: String,
    pub mongo_uri: String,
    pub string_sessions: Vec<String>,
    pub db_name: String,
    pub session_type: String,
    pub logger_id: i64,
    pub owner_id: i64,
    pub owner_username: String,
    pub dev_url: String,
    pub disable_colour: bool,
    pub meow_api_url: String,
    pub meow_api_key: String,
    pub yuki_api_url: String,
    pub yuki_api_key: String,
    pub iota_api_url: String,
    pub iota_api_key: String,
    pub default_lang: String,
    pub duration_limit: i64,
    pub leave_on_demoted: bool,
    pub queue_limit: i64,
    pub support_chat: String,
    pub support_channel: String,
    pub cookies_link: String,
    pub set_cmds: bool,
    pub max_auth_users: i64,
    pub start_images: Vec<String>,
    pub ping_image: String,
    pub repo_image: String,
    pub port: String,
    pub start_time: Instant,
}

fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn getenv_str(key: &str, def: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| def.to_string())
}

fn getenv_i64(key: &str, def: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(def)
}

/// Minimal .env loader: KEY=VALUE lines, quotes stripped, existing env wins
fn load_dotenv(path: &str) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let mut v = v.trim().to_string();
        if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
            v = v[1..v.len() - 1].to_string();
        }
        if std::env::var_os(k).is_none() {
            // Safety: load_dotenv runs before any thread that reads env spawns;
            // callers serialize concurrent load() invocations.
            unsafe {
                std::env::set_var(k, v);
            }
        }
    }
}

/// Fallback for handlers invoked without `init` (tests): env-free defaults
/// with the process start clock running.
impl Default for Config {
    fn default() -> Config {
        Config {
            api_id: 0,
            api_hash: String::new(),
            token: String::new(),
            mongo_uri: String::new(),
            string_sessions: Vec::new(),
            db_name: String::new(),
            session_type: String::new(),
            logger_id: 0,
            owner_id: 0,
            owner_username: String::new(),
            dev_url: String::new(),
            disable_colour: false,
            meow_api_url: String::new(),
            meow_api_key: String::new(),
            yuki_api_url: String::new(),
            yuki_api_key: String::new(),
            iota_api_url: String::new(),
            iota_api_key: String::new(),
            default_lang: String::new(),
            duration_limit: 0,
            leave_on_demoted: false,
            queue_limit: 0,
            support_chat: String::new(),
            support_channel: String::new(),
            cookies_link: String::new(),
            set_cmds: false,
            max_auth_users: 10,
            start_images: Vec::new(),
            ping_image: String::new(),
            repo_image: String::new(),
            port: String::new(),
            start_time: std::time::Instant::now(),
        }
    }
}
// Deviation note: uses a Config singleton; Rust passes &Config explicitly (DI).
pub fn load() -> Result<Config, String> {
    load_from(".env")
}

/// load with a caller-chosen dotenv path; pass "" to skip dotenv entirely
/// (test seam — env vars are provided by the test harness instead).
pub fn load_from(dotenv: &str) -> Result<Config, String> {
    if !dotenv.is_empty() {
        load_dotenv(dotenv);
    }

    let cfg = Config {
        api_id: getenv_i64("API_ID", 0) as i32,
        api_hash: getenv_str("API_HASH", ""),
        token: {
            let t = getenv_str("TOKEN", "");
            if t.is_empty() {
                getenv_str("BOT_TOKEN", "")
            } else {
                t
            }
        },
        mongo_uri: getenv_str("MONGO_DB_URI", ""),
        string_sessions: {
            let raw = getenv_str("STRING_SESSIONS", "");
            let raw = if raw.is_empty() {
                getenv_str("STRING_SESSION", "")
            } else {
                raw
            };
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        },
        db_name: getenv_str("DB_NAME", "ParvatiMusic"),
        session_type: getenv_str("SESSION_TYPE", "271"),
        logger_id: {
            let l = getenv_i64("LOGGER_ID", 0);
            if l != 0 {
                l
            } else {
                getenv_i64("LOG_GROUP_ID", 0)
            }
        },
        owner_id: getenv_i64("OWNER_ID", 0),
        owner_username: getenv_str("OWNER_USERNAME", ""),
        dev_url: {
            let d = getenv_str("DEV_URL", "");
            if d.is_empty() {
                getenv_str("DEVELOPER_URL", "")
            } else {
                d
            }
        },
        disable_colour: parse_bool(&getenv_str("DISABLE_COLOUR", "")),
        meow_api_url: getenv_str("MEOW_API_URL", ""),
        meow_api_key: getenv_str("MEOW_API_KEY", ""),
        yuki_api_url: {
            let y = getenv_str("YUKI_API_URL", "");
            if y.is_empty() {
                getenv_str("MEOW_API_URL", "")
            } else {
                y
            }
        },
        yuki_api_key: {
            let y = getenv_str("YUKI_API_KEY", "");
            if y.is_empty() {
                getenv_str("MEOW_API_KEY", "")
            } else {
                y
            }
        },
        iota_api_url: getenv_str("IOTA_API_URL", "http://127.0.0.1:8000"),
        iota_api_key: getenv_str("IOTA_API_KEY", ""),
        default_lang: getenv_str("DEFAULT_LANG", "en"),
        duration_limit: getenv_i64("DURATION_LIMIT", 9000),
        leave_on_demoted: parse_bool(&getenv_str("LEAVE_ON_DEMOTED", "")),
        queue_limit: getenv_i64("QUEUE_LIMIT", 500),
        support_chat: getenv_str("SUPPORT_CHAT", "https://t.me/+Q493A_VBAUpkMmRl"),
        support_channel: getenv_str("SUPPORT_CHANNEL", "https://t.me/im_iota"),
        cookies_link: getenv_str("COOKIES_LINK", ""),
        set_cmds: parse_bool(&getenv_str("SET_CMDS", "")),
        max_auth_users: getenv_i64("MAX_AUTH_USERS", 10),
        start_images: {
            let raw = getenv_str("START_IMAGES", "");
            let raw = if raw.is_empty() {
                getenv_str("START_IMG_URL", "")
            } else {
                raw
            };
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        },
        ping_image: getenv_str("PING_IMG_URL", ""),
        repo_image: getenv_str("REPO_IMG_URL", ""),
        port: getenv_str("PORT", "8000"),
        start_time: Instant::now(),
    };

    for (name, val) in [
        ("API_ID", cfg.api_id != 0),
        ("API_HASH", !cfg.api_hash.is_empty()),
        ("TOKEN", !cfg.token.is_empty()),
        ("MONGO_DB_URI", !cfg.mongo_uri.is_empty()),
        ("STRING_SESSIONS", !cfg.string_sessions.is_empty()),
    ] {
        if !val {
            return Err(format!("config: missing required env {name}"));
        }
    }
    Ok(cfg)
}

impl Config {
    pub fn start_image(&self) -> &str {
        match self.start_images.is_empty() {
            false => &self.start_images[0],
            true => "",
        }
    }
}
