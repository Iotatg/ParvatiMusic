//! Program bootstrap: config -> cache/download dirs -> db init -> locales
//! (runtime dir first, checkout-vendored fallback) -> assistants + indexes
//! -> platforms -> bot login + module wiring -> HTTP health thread -> run
//! forever. Signal handling is left to the platform (std has no portable
//! handler without extra crates): the pump loop runs until the process is
//! stopped.

use parvati_bot::assistant::AssistantManager;
use parvati_bot::buttons as buttons_module;
use parvati_bot::config;
use parvati_bot::db;
use parvati_bot::ferogram_bot::FerogramBot;
use parvati_bot::locales::Locales;
use parvati_bot::logger::{LogLevel, Logger};
use parvati_bot::mod_tg::Bot;
use parvati_bot::monitor;
use parvati_bot::platforms;
use parvati_bot::room::Rooms;
use parvati_bot::{health, modules};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Each pump_step blocks up to this long reading the socket; the event bus is
// drained right after. Reference bots (TDLib) react in tens of ms via push
// updates, so keep this short instead of holding updates for a whole window.
const PUMP_IDLE: Duration = Duration::from_millis(200);

fn fatal(msg: &str) -> ! {
    Logger::instance().log(LogLevel::Fatal, "main", msg);
    unreachable!("Logger::log(Fatal) aborts the process");
}

fn main() {
    // (2) config
    let cfg = config::load().unwrap_or_else(|e| fatal(&e));

    // (3) cache/ + downloads/ dirs
    for d in ["cache", "downloads"] {
        if let Err(e) = std::fs::create_dir_all(d) {
            fatal(&format!("failed to refresh directories: {e}"));
        }
    }

    // (4) db::init (Rust seeds the owner id used by admin command gates)
    db::init(cfg.owner_id);

    // (5) locales: "locales" first, checkout-vendored fallback
    let loc = Locales::load("locales")
        .or_else(|_| {
            Locales::load(concat!(env!("CARGO_MANIFEST_DIR"), "/locales"))
        })
        .unwrap_or_else(|e| fatal(&e));

    // (6) assistants from string sessions + db index counter
    let assists = Arc::new(
        AssistantManager::from_sessions(&cfg).unwrap_or_else(|e| fatal(&e)),
    );
    db::init_assistant_indexes(assists.count() as i64);

    // (7) platform registry
    platforms::registry::init(&cfg);

    // (8) bot login (FerogramBot) + module/router wiring
    let mut tg = FerogramBot::new(&cfg.token)
        .unwrap_or_else(|e| fatal(&format!("bot login failed: {e}")));
    // start_markup builds https://t.me/{u}?startgroup buttons; without the
    // username they'd carry an empty channel and the server rejects the
    // whole markup (BUTTON_URL_INVALID).
    buttons_module::set_bot_username(&tg.me_username());
    buttons_module::set_disable_colour(cfg.disable_colour);
    let rooms = Rooms::new();
    let rx = modules::init(&mut tg, assists.clone(), rooms.clone(), loc, cfg.clone());

    // bot becomes the shared handle monitor threads and the pump loop
    // lock around.
    let bot: Arc<Mutex<Box<dyn Bot>>> = Arc::new(Mutex::new(Box::new(tg)));
    let loc_arc = modules::locales_arc().expect("init installed locales");

    // (11) room monitor + stream-end threads
    monitor::start_room_monitor(rooms.clone(), assists.clone(), bot.clone(), loc_arc.clone());
    let stream_ends = monitor::wire_stream_end(&assists);
    monitor::run_stream_end(stream_ends, rooms, assists, bot.clone(), loc_arc);

    // (9) HTTP health server (fixed 200 OK on any connection)
    if let Err(e) = health::spawn(&cfg.port) {
        Logger::instance().log(
            LogLevel::Error,
            "http",
            &format!("health server failed to bind: {e}"),
        );
    }

    Logger::instance().log(LogLevel::Info, "main", "parvati music ready");

    // (12) pump forever on the main thread: each cycle the
    // client consumes incoming mtproto messages (updates already wired to
    // the module bus); drained events dispatch through the registry.
    loop {
        let step = {
            let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
            b.pump_step(PUMP_IDLE)
        };
        if let Err(e) = step {
            Logger::instance().log(
                LogLevel::Warn,
                "pump",
                &format!("connection lost ({e}); reconnecting"),
            );
            let mut tries = 0u32;
            loop {
                tries += 1;
                let retry = {
                    let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
                    b.reconnect()
                };
                match retry {
                    Ok(()) => {
                        Logger::instance().log(
                            LogLevel::Info,
                            "pump",
                            &format!("reconnected after {tries} attempt(s)"),
                        );
                        break;
                    }
                    Err(re) => {
                        Logger::instance().log(
                            LogLevel::Error,
                            "pump",
                            &format!("reconnect attempt {tries} failed: {re}"),
                        );
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        }
        while let Ok(ev) = rx.try_recv() {
            let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(reg) = modules::registry() {
                reg.dispatch(&ev, &mut **b);
            }
        }
    }
}
