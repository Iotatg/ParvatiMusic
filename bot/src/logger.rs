use std::io::Write;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    fn tag(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO ",
            LogLevel::Warn => "WARN ",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }
}

pub struct Logger {
    level: Mutex<LogLevel>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

impl Logger {
    pub fn instance() -> &'static Logger {
        LOGGER.get_or_init(|| Logger {
            level: Mutex::new(LogLevel::Info),
        })
    }

    pub fn set_level(&self, l: LogLevel) {
        *self.level.lock().unwrap() = l;
    }

    pub fn log(&self, level: LogLevel, tag: &str, msg: &str) {
        if level < *self.level.lock().unwrap() {
            return;
        }
        let t = chrono_free_now();
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{t} [{}] {tag}: {msg}", level.tag());
        if level == LogLevel::Fatal {
 // parity: std::abort on fatal.
            std::process::abort();
        }
    }
}

/// HH:MM:SS from SystemTime without external crates (std only).
fn chrono_free_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tod = secs % 86_400;
    format!("{:02}:{:02}:{:02}", tod / 3600, (tod % 3600) / 60, tod % 60)
}

#[macro_export]
macro_rules! parv_debug {
    ($tag:expr, $($arg:tt)*) => {
        $crate::logger::Logger::instance().log($crate::logger::LogLevel::Debug, $tag, &format!($($arg)*))
    };
}
#[macro_export]
macro_rules! parv_info {
    ($tag:expr, $($arg:tt)*) => {
        $crate::logger::Logger::instance().log($crate::logger::LogLevel::Info, $tag, &format!($($arg)*))
    };
}
#[macro_export]
macro_rules! parv_warn {
    ($tag:expr, $($arg:tt)*) => {
        $crate::logger::Logger::instance().log($crate::logger::LogLevel::Warn, $tag, &format!($($arg)*))
    };
}
#[macro_export]
macro_rules! parv_error {
    ($tag:expr, $($arg:tt)*) => {
        $crate::logger::Logger::instance().log($crate::logger::LogLevel::Error, $tag, &format!($($arg)*))
    };
}
