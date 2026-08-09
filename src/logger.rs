use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

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

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::logger::Logger::log("DEBUG", &format!($($arg)*));
    };
}

#[allow(dead_code)]
pub struct Logger {
    file: File,
    pub path: PathBuf,
}

impl Logger {
    pub fn init() {
        let mut log_path = match std::env::var("LOCALAPPDATA") {
            Ok(appdata) => std::path::PathBuf::from(appdata)
                .join("WinSpaces")
                .join("winspaces.log"),
            Err(_) => std::path::PathBuf::from("winspaces.log"),
        };

        if let Ok(mut exe_dir) = std::env::current_exe() {
            exe_dir.pop();
            let portable_log = exe_dir.join("winspaces.log");
            if exe_dir.join("settings.json").exists() || portable_log.exists() {
                log_path = portable_log;
            }
        }

        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let mut guard = LOGGER.lock().unwrap();
            *guard = Some(Logger {
                file,
                path: log_path.clone(),
            });
        }

        log_info!("==========================================");
        log_info!("WinSpaces started. Log initialized at: {:?}", log_path);
    }

    pub fn log(level: &str, msg: &str) {
        let timestamp = get_timestamp_string();
        let formatted = format!("[{}] [{}] {}\n", timestamp, level, msg);

        if let Ok(mut guard) = LOGGER.lock() {
            if let Some(logger) = guard.as_mut() {
                let _ = logger.file.write_all(formatted.as_bytes());
                let _ = logger.file.flush();
            }
        }
    }
}

fn get_timestamp_string() -> String {
    unsafe {
        let mut st = std::mem::zeroed();
        GetLocalTime(&mut st);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
}
