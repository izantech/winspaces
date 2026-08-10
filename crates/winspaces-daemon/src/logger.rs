use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOGGER: Mutex<Option<File>> = Mutex::new(None);

pub struct Logger;

/// Rotate the log once it exceeds this size; one `.old` generation is kept.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

impl Logger {
    pub fn init() {
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

    pub fn log(level: &str, msg: &str) {
        let mut guard = LOGGER.lock().unwrap();
        if let Some(ref mut file) = *guard {
            let timestamp = format_timestamp();
            let line = format!("[{}] [{}] {}\n", timestamp, level, msg);
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
        $crate::logger::Logger::log("INFO", &format!($($arg)*));
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::logger::Logger::log("WARN", &format!($($arg)*));
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logger::Logger::log("ERROR", &format!($($arg)*));
    };
}
