//! /start, /help and /ping commands plus their callback handlers.
use std::time::Instant;

use parvati_mtproto::messaging::{TgCallback, TgMessage};

use crate::buttons;
use crate::locales::LocaleArg;
use crate::modules::common::f;
use crate::modules::help_texts::format_help_text;
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;
use crate::utils::{escape_html, format_uptime, html_mention, trim};

/// + callback registrations for this file.
pub fn register(r: &mut Registry) {
    r.command("start", handle_start);
    r.command("help", handle_help);
    r.command("ping", handle_ping);
    r.command("stats", handle_stats);
    r.callback("start", handle_start_cb);
    r.callback("help_cb", handle_help_cb);
    r.callback("help:", handle_help_page_cb);
}

/// mention_of.
fn mention_of(m: &TgMessage) -> String {
    let full = format!("{} {}", m.first_name, m.last_name);
    let name = trim(&full);
    html_mention(if name.is_empty() { "user" } else { name }, m.sender_id)
}

/// handle_start.
pub fn handle_start(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    let loc = loc();
    if !m.is_private {
        crate::db::add_served_chat(m.chat_id);
        let text = f(loc, m.chat_id, "start_group");
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    crate::db::add_served_user(m.chat_id);
    let lang = crate::db::language(m.chat_id);
    let mut a = LocaleArg::new();
    a.insert("user".into(), mention_of(m));
    a.insert("bot".into(), html_mention(&bot.me_first_name(), bot.me_id()));
    let caption = loc.get(&lang, "start_private", &a);
    let cfg = crate::modules::cfg();
    let mk = buttons::start_markup(&cfg, loc, &lang);
    if let Some(image) = cfg.start_images.first() {
        let _ = bot.send_photo(m.chat_id, image, &caption, Some(&mk));
        return;
    }
    let _ = bot.send_text(m.chat_id, &caption, Some(&mk));
}

/// handle_start_cb.
pub fn handle_start_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let _ = bot.answer_callback(cb.query_id, "", false);
    let loc = loc();
    let lang = crate::db::language(cb.chat_id);
    let full = format!("{} {}", cb.first_name, cb.last_name);
    let name = trim(&full);
    let mut a = LocaleArg::new();
    a.insert("user".into(), html_mention(if name.is_empty() { "user" } else { name }, cb.user_id));
    a.insert("bot".into(), html_mention(&bot.me_first_name(), bot.me_id()));
    let caption = loc.get(&lang, "start_private", &a);
    let cfg = crate::modules::cfg();
    let mk = buttons::start_markup(&cfg, loc, &lang);
    let _ = bot.edit_text(cb.chat_id, cb.message_id, &caption, Some(&mk));
}

/// fields.
fn fields(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// show_help_for.
fn show_help_for(bot: &mut dyn Bot, m: &TgMessage, cmd: &str) {
    let mut help_text = format_help_text(cmd);
    if help_text.is_empty() {
        let trimmed = cmd.strip_prefix('/').unwrap_or(cmd);
        help_text = format_help_text(trimmed);
    }
    if help_text.is_empty() {
        let _ = bot.reply_text(
            m,
            &format!("⚠️ <i>No help found for command <code>{}</code></i>", escape_html(cmd)),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &format!("📘 <b>Help for</b> <code>{}</code>:\n\n{}", escape_html(cmd), help_text),
        None,
    );
}

/// handle_help.
pub fn handle_help(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    let loc = loc();
    let args = fields(&m.text);
    if args.len() > 1 && args[1] != "pm_help" {
        let mut cmd = args[1].clone();
        if cmd.is_empty() || !cmd.starts_with('/') {
            cmd = format!("/{cmd}");
        }
        show_help_for(bot, m, &cmd);
        return;
    }
    if !m.is_private {
        let text = f(loc, m.chat_id, "help_private_only");
        let lang = crate::db::language(m.chat_id);
        let _ = bot.reply_text(m, &text, Some(&buttons::group_help_keyboard(loc, &lang)));
        return;
    }
    let lang = crate::db::language(m.chat_id);
    let _ = bot.reply_text(m, &f(loc, m.chat_id, "help_main"), Some(&buttons::help_keyboard(loc, &lang)));
}

/// handle_help_cb.
pub fn handle_help_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let _ = bot.answer_callback(cb.query_id, "", false);
    let loc = loc();
    let lang = crate::db::language(cb.chat_id);
    let _ = bot.edit_text(cb.chat_id, cb.message_id, &f(loc, cb.chat_id, "help_main"), Some(&buttons::help_keyboard(loc, &lang)));
}

/// handle_help_page_cb.
pub fn handle_help_page_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let _ = bot.answer_callback(cb.query_id, "", false);
    if cb.data.is_empty() {
        return;
    }
    let Some(sep) = cb.data.find(':') else { return };
    let page = &cb.data[sep + 1..];
    let loc = loc();
    let lang = crate::db::language(cb.chat_id);
    let mut mk = buttons::back_keyboard(loc, &lang);
    let text: String;
    match page {
        "admin" => text = f(loc, cb.chat_id, "help_admin"),
        "auth" => text = f(loc, cb.chat_id, "help_auth"),
        "bcast" => text = f(loc, cb.chat_id, "help_bcast"),
        "play" => text = f(loc, cb.chat_id, "help_play"),
        "sudo" => text = f(loc, cb.chat_id, "help_sudo"),
        "restrict" => text = f(loc, cb.chat_id, "help_restrict"),
        "thumb" => text = f(loc, cb.chat_id, "help_thumb"),
        "start" => text = f(loc, cb.chat_id, "help_start"),
        "autoplay" => text = f(loc, cb.chat_id, "help_autoplay"),
        "playlist" => text = f(loc, cb.chat_id, "help_playlist"),
        "vclogs" => text = f(loc, cb.chat_id, "help_vclogs"),
        "inline" => text = f(loc, cb.chat_id, "help_inline"),
        "main" => {
            text = f(loc, cb.chat_id, "help_main");
            mk = buttons::help_keyboard(loc, &lang);
        }
        _ => return,
    }
    let _ = bot.edit_text(cb.chat_id, cb.message_id, &text, Some(&mk));
}

/// ram_info.
fn ram_info() -> String {
    let (used, total) = ram_values();
    if total == 0.0 {
        return "N/A".into();
    }
    format!("{used:.2} / {total:.2} GB")
}

/// (used_gib, total_gib) from /proc/meminfo.
fn ram_values() -> (f64, f64) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return (0.0, 0.0);
    };
    let mut mem_total = 0i64;
    let mut mem_avail = 0i64;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(k) = it.next() else { continue };
        let Some(v) = it.next().and_then(|v| v.parse::<i64>().ok()) else { continue };
        match k {
            "MemTotal:" => mem_total = v,
            "MemAvailable:" => mem_avail = v,
            _ => continue,
        }
        if mem_total != 0 && mem_avail != 0 {
            break;
        }
    }
    if mem_total == 0 {
        return (0.0, 0.0);
    }
    let used = (mem_total - mem_avail) as f64 / 1024.0 / 1024.0;
    let total = mem_total as f64 / 1024.0 / 1024.0;
    (used, total)
}

/// disk_info (statvfs("/")).
fn disk_info() -> String {
    let (used, total) = disk_values();
    if total == 0.0 {
        return "N/A".into();
    }
    format!("{used:.2} / {total:.2} GB")
}

/// (used_gib, total_gib) from statvfs("/").
fn disk_values() -> (f64, f64) {
    let Ok(path) = std::ffi::CString::new("/") else {
        return (0.0, 0.0);
    };
    // SAFETY: zeroed statvfs + valid NUL-terminated path; no aliasing.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(path.as_ptr(), &mut st) };
    if rc != 0 {
        return (0.0, 0.0);
    }
    let total = st.f_blocks as f64 * st.f_frsize as f64 / 1024.0 / 1024.0 / 1024.0;
    let free = st.f_bavail as f64 * st.f_frsize as f64 / 1024.0 / 1024.0 / 1024.0;
    (total - free, total)
}

/// cpu_info.
fn cpu_info() -> String {
    format!("{:.2}%", cpu_info_value())
}

/// CPU load percent. Prefer the cgroup v2 `cpu.stat` delta (real container CPU
/// usage); fall back to loadavg / ncpu when cgroup accounting is unavailable.
fn cpu_info_value() -> f64 {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static PREV: OnceLock<Mutex<Option<(u64, Instant)>>> = OnceLock::new();
    let now = Instant::now();
    if let Some(usage) = cgroup_cpu_usage_usec() {
        let cell = PREV.get_or_init(|| Mutex::new(None));
        let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
        let pct = match *guard {
            Some((prev_usage, prev_at)) => {
                let dt = now.duration_since(prev_at).as_secs_f64();
                if dt > 0.0 && usage >= prev_usage {
                    (usage - prev_usage) as f64 / 1_000_000.0 / dt * 100.0
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        *guard = Some((usage, now));
        return pct;
    }
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|t| t.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(0.0);
    let n = std::thread::available_parallelism().map(|n| n.get() as i64).unwrap_or(1).max(1);
    load / n as f64 * 100.0
}

/// Cumulative CPU time of the current cgroup in microseconds (cgroup v2).
fn cgroup_cpu_usage_usec() -> Option<u64> {
    let text = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("usage_usec "))
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// handle_ping.
pub fn handle_ping(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    if m.is_private {
        let _ = bot.delete_message(m.chat_id, m.id);
        crate::db::add_served_user(m.chat_id);
    } else {
        crate::db::add_served_chat(m.chat_id);
    }

    let loc = loc();
    let lang = crate::db::language(m.chat_id);
    let start = Instant::now();
    let reply_id = bot
        .send_text_sync(m.chat_id, &f(loc, m.chat_id, "ping_start"), None, if m.is_private { 0 } else { m.id })
        .unwrap_or(0);
    let latency = start.elapsed().as_millis() as i64;
    let uptime = crate::modules::cfg().start_time.elapsed().as_secs() as i64;
    let mut a = LocaleArg::new();
    a.insert("latency".into(), latency.to_string());
    a.insert("bot".into(), html_mention(&bot.me_first_name(), bot.me_id()));
    a.insert("uptime".into(), format_uptime(uptime));
    a.insert("ram_info".into(), ram_info());
    a.insert("cpu_usage".into(), cpu_info());
    a.insert("disk_usage".into(), disk_info());
    let msg = loc.get(&lang, "ping_result", &a);
    let mk = buttons::supp_markup(&crate::modules::cfg(), loc, &lang);
    if reply_id != 0 {
        if !bot.edit_text(m.chat_id, reply_id, &msg, Some(&mk)).unwrap_or(false) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = bot.edit_text(m.chat_id, reply_id, &msg, Some(&mk));
        }
    } else {
        let _ = bot.send_text(m.chat_id, &msg, Some(&mk));
    }
}

/// Process RSS / virtual size in MiB from /proc/self/statm (Rust has no GC;
/// these stand in for the reference's runtime memory counters).
fn process_mem_mib() -> (f64, f64) {
    let Ok(text) = std::fs::read_to_string("/proc/self/statm") else {
        return (0.0, 0.0);
    };
    let mut it = text.split_whitespace();
    let _size = it.next();
    let resident = it.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let page = 4096.0;
    let rss = resident * page / 1024.0 / 1024.0;
    (rss, 0.0)
}

/// Live OS-thread count from /proc/self/status (reference shows goroutines).
fn thread_count() -> i64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("Threads:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<i64>().ok()))
        })
        .unwrap_or(0)
}

fn emoji_level(value: f64, warn: f64, crit: f64) -> &'static str {
    if value > crit {
        "\u{1F534}" // red
    } else if value > warn {
        "\u{1F7E1}" // yellow
    } else {
        "\u{1F7E2}" // green
    }
}

/// handle_stats (system overview; runs before private/group bookkeeping).
pub fn handle_stats(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    if !(crate::modules::admin::caller_is_owner_or_sudo(m)) {
        return;
    }
    let loc = loc();
    let lang = crate::db::language(m.chat_id);

    let (rss_mib, _) = process_mem_mib();
    let cpu = cpu_info_value();
    let (ram_used, ram_total) = ram_values();
    let (disk_used, disk_total) = disk_values();
    let ram_pct = if ram_total > 0.0 { ram_used / ram_total * 100.0 } else { 0.0 };

    let mut a = LocaleArg::new();
    a.insert("os".into(), std::env::consts::OS.to_string());
    a.insert("arch".into(), std::env::consts::ARCH.to_string());
    a.insert(
        "cpus".into(),
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).to_string(),
    );
    a.insert("goroutines".into(), thread_count().to_string());
    a.insert("alloc".into(), format!("{rss_mib:.2}"));
    a.insert("sys".into(), format!("{rss_mib:.2}"));
    a.insert("gc_count".into(), "0".into());
    a.insert("gc_emoji".into(), "\u{1F7E2}".into());
    a.insert("gc_rate".into(), "0.0".into());
    a.insert("cpu".into(), format!("{cpu:.2}"));
    a.insert("cpu_emoji".into(), emoji_level(cpu, 40.0, 70.0).into());
    a.insert("ram_emoji".into(), emoji_level(ram_pct, 50.0, 80.0).into());
    a.insert("ram_used_gib".into(), format!("{ram_used:.2}"));
    a.insert("ram_total_gib".into(), format!("{ram_total:.2}"));
    a.insert("storage_used_gib".into(), format!("{disk_used:.2}"));
    a.insert("storage_total_gib".into(), format!("{disk_total:.2}"));
    a.insert("db_data".into(), "0.00".into());
    a.insert("db_storage".into(), "0.00".into());
    a.insert("db_collections".into(), "0".into());
    a.insert("db_objects".into(), crate::db::served_users().len().to_string());
    a.insert("served_chats".into(), crate::db::served_chats().len().to_string());
    a.insert("served_users".into(), crate::db::served_users().len().to_string());
    a.insert("go_version".into(), env!("RUSTC_VERSION").to_string());
    a.insert("gogram_api_layer".into(), "MTProto layer 227".into());
    a.insert("ntgcalls_version".into(), "ntgcalls-rs".into());
    let msg = loc.get(&lang, "stats_overview", &a);
    let _ = bot.send_text(m.chat_id, &msg, None);
}
