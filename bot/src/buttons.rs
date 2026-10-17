// buttons — reply markups.
// with JSON shapes mapped onto parvati_mtproto::messaging::{Button, Markup}.

use parvati_mtproto::messaging::{Btn, Button, Markup};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::locales::Locales;
use crate::utils::{base64_decode, base64_encode, format_time, get_progress_bar};

static BOT_USERNAME: Mutex<Option<String>> = Mutex::new(None);
static DISABLE_COLOUR: AtomicBool = AtomicBool::new(false);

pub fn set_bot_username(u: &str) {
    *BOT_USERNAME.lock().unwrap() = Some(u.to_string());
}

/// Mirrors DISABLE_COLOUR in the reference bots: when set, buttons carry no
/// colour and fall back to the client default (style 0).
pub fn set_disable_colour(v: bool) {
    DISABLE_COLOUR.store(v, Ordering::Relaxed);
}

pub fn bot_username() -> String {
    BOT_USERNAME.lock().unwrap().clone().unwrap_or_default()
}

/// Random button colour, parity with AdvaitMusic/KanhaMusic: each button
/// independently gets red/blue/green, unless DISABLE_COLOUR is set.
/// style: 1 primary(blue), 2 danger(red), 3 success(green).
fn random_style() -> u8 {
    if DISABLE_COLOUR.load(Ordering::Relaxed) {
        return 0;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos ^ 0x9E3779B97F4A7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % 3) as u8 + 1
}

/// url_btn/btn_cb (callback payload = base64_encode(cb)).
/// Colour is random per button (see random_style).
pub fn url_btn(text: &str, url: &str) -> Button {
    Button {
        text: text.into(),
        kind: Btn::Url(url.into()),
        style: random_style(),
    }
}

pub fn data_btn(text: &str, cb: &str) -> Button {
    Button {
        text: text.into(),
        kind: Btn::Callback(base64_encode(cb.as_bytes()).into_bytes()),
        style: random_style(),
    }
}

pub fn url_btn_styled(text: &str, url: &str, style: u8) -> Button {
    Button {
        style,
        ..url_btn(text, url)
    }
}

pub fn data_btn_styled(text: &str, cb: &str, style: u8) -> Button {
    Button {
        style,
        ..data_btn(text, cb)
    }
}

pub fn decode_btn_data(data: &str) -> String {
    base64_decode(data)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

pub fn rows(r: Vec<Vec<Button>>) -> Markup {
    Markup { rows: r }
}

/// The subset of RoomState that markups render (Task 8 defines the real thing).
pub struct RoomView {
    pub id: i64,
    pub paused: bool,
    pub autoplay: bool,
    pub position: i64,
    pub duration: i64,
}

/// (play_markup): progress row only when !queued.
pub fn play_markup(loc: &Locales, lang: &str, r: &RoomView, queued: bool) -> Markup {
    let prefix = format!("room:{}:", r.id);
    let cb = |action: &str, text: String| data_btn(&text, &format!("{prefix}{action}"));
    let progress = format!(
        "{} {} {}",
        format_time(r.position),
        get_progress_bar(r.position, r.duration),
        format_time(r.duration)
    );
    let mut all: Vec<Vec<Button>> = Vec::new();
    if !queued {
        all.push(vec![data_btn(&progress, "progress")]);
    }
    all.push(vec![
        cb("resume", "\u{25B7}".into()),
        cb("pause", "II".into()),
        cb("replay", "\u{27F3}".into()),
        cb("skip", "\u{2023}\u{2023}I".into()),
        cb("stop", "\u{25A2}".into()),
    ]);
    all.push(vec![
        cb("seekback_15", "-\u{1D7E3}\u{1D7E7}\u{02E2}".into()),
        cb("seek_15", "\u{1D7E3}\u{1D7E7}\u{02E2}+".into()),
    ]);
    let state = if r.autoplay { "enabled" } else { "disabled" };
    let mut args = crate::locales::LocaleArg::new();
    args.insert(
        "state".into(),
        loc.get(lang, state, &crate::locales::LocaleArg::new()),
    );
    let ap_text = loc.get(lang, "autoplay_btn", &args);
    all.push(vec![cb("autoplay_toggle", ap_text)]);
    all.push(vec![data_btn(&loc.get(lang, "CLOSE_BTN", &crate::locales::LocaleArg::new()), "close")]);
    rows(all)
}

/// (stop_confirm_markup).
pub fn stop_confirm_markup(loc: &Locales, lang: &str, r: &RoomView) -> Markup {
    let prefix = format!("room:{}:", r.id);
    let (label, action) = if r.paused {
        ("CONFIRM_RESUME_BTN", "resume")
    } else {
        ("CONFIRM_UNMUTE_BTN", "unmute")
    };
    rows(vec![vec![
        data_btn(&loc.get(lang, label, &crate::locales::LocaleArg::new()), &format!("{prefix}{action}")),
        data_btn(&loc.get(lang, "CONFIRM_STOP_BTN", &crate::locales::LocaleArg::new()), &format!("{prefix}stop")),
    ]])
}

/// (add_me_markup).
pub fn add_me_markup(loc: &Locales, lang: &str) -> Markup {
    rows(vec![vec![url_btn(
        &loc.get(lang, "ADD_ME_BTN", &crate::locales::LocaleArg::new()),
        &format!("https://t.me/{}?startgroup&admin=invite_users", bot_username()),
    )]])
}

/// (cancel_keyboard).
pub fn cancel_keyboard(loc: &Locales, lang: &str) -> Markup {
    rows(vec![vec![data_btn(
        &loc.get(lang, "DOWNLOAD_CANCEL_BTN", &crate::locales::LocaleArg::new()),
        "cancel",
    )]])
}

/// (broadcast_cancel_keyboard).
pub fn broadcast_cancel_keyboard(loc: &Locales, lang: &str) -> Markup {
    rows(vec![vec![data_btn(
        &loc.get(lang, "BROADCAST_CANCEL_BTN", &crate::locales::LocaleArg::new()),
        "bcast_cancel",
    )]])
}

/// (supp_markup).
pub fn supp_markup(cfg: &Config, loc: &Locales, lang: &str) -> Markup {
    rows(vec![vec![url_btn(
        &loc.get(lang, "SUPPORT_BTN", &crate::locales::LocaleArg::new()),
        &cfg.support_chat,
    )]])
}

/// (group_help_keyboard).
pub fn group_help_keyboard(loc: &Locales, lang: &str) -> Markup {
    rows(vec![vec![url_btn(
        &loc.get(lang, "GC_HELP_BTN", &crate::locales::LocaleArg::new()),
        &format!("https://t.me/{}?start=pm_help", bot_username()),
    )]])
}

/// (help_keyboard, 4 topic rows + back).
pub fn help_keyboard(loc: &Locales, lang: &str) -> Markup {
    let key = |k: &str| loc.get(lang, k, &crate::locales::LocaleArg::new());
    rows(vec![
        vec![
            data_btn(&key("HELP_ADMIN_BTN"), "help:admin"),
            data_btn(&key("HELP_AUTH_BTN"), "help:auth"),
            data_btn(&key("HELP_BCAST_BTN"), "help:bcast"),
        ],
        vec![
            data_btn(&key("HELP_PLAY_BTN"), "help:play"),
            data_btn(&key("HELP_SUDO_BTN"), "help:sudo"),
            data_btn(&key("HELP_RESTRICT_BTN"), "help:restrict"),
        ],
        vec![
            data_btn(&key("HELP_THUMB_BTN"), "help:thumb"),
            data_btn(&key("HELP_START_BTN"), "help:start"),
            data_btn(&key("HELP_AUTOPLAY_BTN"), "help:autoplay"),
        ],
        vec![
            data_btn(&key("HELP_PLAYLIST_BTN"), "help:playlist"),
            data_btn(&key("HELP_VCLOGS_BTN"), "help:vclogs"),
            data_btn(&key("HELP_INLINE_BTN"), "help:inline"),
        ],
        vec![data_btn(&key("BACK_BTN"), "start")],
    ])
}

/// (back_keyboard).
pub fn back_keyboard(loc: &Locales, lang: &str) -> Markup {
    rows(vec![vec![data_btn(
        &loc.get(lang, "BACK_BTN", &crate::locales::LocaleArg::new()),
        "help:main",
    )]])
}

/// (start_markup, 4 rows). Colours are random per button (reference parity).
pub fn start_markup(cfg: &Config, loc: &Locales, lang: &str) -> Markup {
    let u = bot_username();
    let key = |k: &str| loc.get(lang, k, &crate::locales::LocaleArg::new());
    rows(vec![
        vec![url_btn(
            &key("ADD_ME_BTN"),
            &format!("https://t.me/{u}?startgroup&admin=invite_users"),
        )],
        vec![data_btn(&key("START_HELP_BTN"), "help_cb")],
        vec![
            url_btn(&key("UPDATES_BTN"), &cfg.support_channel),
            url_btn(&key("SUPPORT_BTN"), &cfg.support_chat),
        ],
        vec![url_btn(
            &key("SOURCE_BTN"),
            "https://github.com/Iotatg/ParvatiMusic",
        )],
    ])
}

/// (repo_markup: dev_url fallback chain).
pub fn repo_markup(cfg: &Config, loc: &Locales, lang: &str, dev_url: &str) -> Markup {
    let mut d = dev_url.to_string();
    if d.is_empty() {
        if !cfg.dev_url.is_empty() {
            d = cfg.dev_url.clone();
        } else if !cfg.owner_username.is_empty() {
            let u = cfg.owner_username.trim_start_matches('@').to_string();
            d = format!("https://t.me/{u}");
        } else if cfg.owner_id != 0 {
            d = format!("tg://openmessage?user_id={}", cfg.owner_id);
        } else {
            d = "https://t.me/+Q493A_VBAUpkMmRl".into();
        }
    }
    rows(vec![vec![
        url_btn(
            &loc.get(lang, "REPO_DEV_BTN", &crate::locales::LocaleArg::new()),
            &d,
        ),
        url_btn(
            &loc.get(lang, "REPO_BTN", &crate::locales::LocaleArg::new()),
            "https://github.com/Iotatg/ParvatiMusic",
        ),
    ]])
}
