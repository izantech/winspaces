use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

static LOGGER: Mutex<Option<File>> = Mutex::new(None);

/// Highest level that gets written. Checked by the `log_*!` macros *before*
/// the format arguments are evaluated, so a disabled line costs one relaxed
/// atomic load and nothing else.
static MAX_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }
}

pub struct Logger;

/// Rotate the log once it exceeds this size; one `.old` generation is kept.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

impl Logger {
    pub fn init() {
        if let Ok(v) = std::env::var("WINSPACES_LOG") {
            let level = match v.to_ascii_lowercase().as_str() {
                "error" => Level::Error,
                "warn" => Level::Warn,
                "debug" => Level::Debug,
                _ => Level::Info,
            };
            MAX_LEVEL.store(level as u8, Ordering::Relaxed);
        }

        let path = Self::get_log_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_LOG_BYTES {
                let old = path.with_extension("log.old");
                let _ = std::fs::remove_file(&old);
                let _ = std::fs::rename(&path, &old);
            }
        }

        let file = OpenOptions::new().create(true).append(true).open(&path);

        match file {
            Ok(f) => {
                let mut guard = LOGGER.lock().unwrap();
                *guard = Some(f);
            }
            Err(e) => {
                eprintln!("Failed to open log file at {:?}: {}", path, e);
            }
        }
    }

    #[inline]
    pub fn enabled(level: Level) -> bool {
        level as u8 <= MAX_LEVEL.load(Ordering::Relaxed)
    }

    pub fn log(level: Level, msg: &str) {
        let mut guard = LOGGER.lock().unwrap();
        if let Some(ref mut file) = *guard {
            let timestamp = format_timestamp();
            let line = format!("[{}] [{}] {}\n", timestamp, level.as_str(), msg);
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn get_log_path() -> PathBuf {
        if let Ok(mut exe_dir) = std::env::current_exe() {
            exe_dir.pop();
            let portable_log = exe_dir.join("winspaces.log");
            if exe_dir.join("settings.json").exists() || portable_log.exists() {
                return portable_log;
            }
        }

        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            let dir = PathBuf::from(appdata).join("WinSpaces");
            let _ = std::fs::create_dir_all(&dir);
            return dir.join("winspaces.log");
        }

        PathBuf::from("winspaces.log")
    }
}

fn format_timestamp() -> String {
    unsafe {
        let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        if $crate::logger::Logger::enabled($crate::logger::Level::Info) {
            $crate::logger::Logger::log($crate::logger::Level::Info, &format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        if $crate::logger::Logger::enabled($crate::logger::Level::Warn) {
            $crate::logger::Logger::log($crate::logger::Level::Warn, &format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        if $crate::logger::Logger::enabled($crate::logger::Level::Error) {
            $crate::logger::Logger::log($crate::logger::Level::Error, &format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if $crate::logger::Logger::enabled($crate::logger::Level::Debug) {
            $crate::logger::Logger::log($crate::logger::Level::Debug, &format!($($arg)*));
        }
    };
}
