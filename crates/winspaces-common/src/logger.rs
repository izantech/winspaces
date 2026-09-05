use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Highest level that gets written. Checked by the `log_*!` macros *before*
/// the format arguments are evaluated, so a disabled line costs one relaxed
/// atomic load and nothing else.
static MAX_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// Rotate the log once it exceeds this size; one `.old` generation is kept.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

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

/// The open log file plus what rotation needs to know about it.
struct Sink {
    file: File,
    path: PathBuf,
    /// Bytes in the file, counted from its size at open time. Checked after
    /// every line, so a daemon that runs for weeks rotates in session instead
    /// of only at its next start.
    written: u64,
}

impl Sink {
    fn open(path: PathBuf) -> std::io::Result<Sink> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Sink {
            file,
            path,
            written,
        })
    }

    /// Append one line; once the file has grown past `max_bytes`, move it to
    /// `<name>.old` and continue in a fresh file. Returns whether it rotated.
    fn write_line(&mut self, line: &str, max_bytes: u64) -> bool {
        let _ = self.file.write_all(line.as_bytes());
        // Not buffered: the line is already with the OS, which is what crash
        // forensics need, and `File::flush` costs nothing.
        let _ = self.file.flush();
        self.written += line.len() as u64;
        if self.written <= max_bytes {
            return false;
        }
        rotate(&self.path);
        match Sink::open(self.path.clone()) {
            Ok(fresh) => *self = fresh,
            // The old handle now points at the renamed file; keep writing there
            // rather than losing lines, and try again at the next threshold.
            Err(_) => self.written = 0,
        }
        true
    }
}

/// `winspaces.log` -> `winspaces.log.old`, replacing the previous generation.
/// Works while the log is open: std opens files with `FILE_SHARE_DELETE`, so
/// the rename succeeds and the open handle follows the renamed file.
fn rotate(path: &Path) {
    let old = path.with_extension("log.old");
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::rename(path, &old);
}

pub struct Logger;

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

        // A file an older build left oversized rotates before the first line.
        if std::fs::metadata(&path)
            .map(|meta| meta.len() > MAX_LOG_BYTES)
            .unwrap_or(false)
        {
            rotate(&path);
        }

        match Sink::open(path.clone()) {
            Ok(sink) => *Self::lock() = Some(sink),
            Err(e) => eprintln!("Failed to open log file at {:?}: {}", path, e),
        }
    }

    /// A panic while the lock is held (only possible in unwinding builds)
    /// must not take every later line down with it, least of all the panic
    /// hook's own.
    fn lock() -> MutexGuard<'static, Option<Sink>> {
        SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[inline]
    pub fn enabled(level: Level) -> bool {
        level as u8 <= MAX_LEVEL.load(Ordering::Relaxed)
    }

    pub fn log(level: Level, msg: &str) {
        let mut guard = Self::lock();
        if let Some(sink) = guard.as_mut() {
            let line = format!("[{}] [{}] {}\n", format_timestamp(), level.as_str(), msg);
            sink.write_line(&line, MAX_LOG_BYTES);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("winspaces-logger-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("log.old"));
        path
    }

    #[test]
    fn rotates_in_session_once_the_cap_is_exceeded() {
        let path = temp_log("rotate.log");
        let mut sink = Sink::open(path.clone()).unwrap();
        assert!(!sink.write_line("first line\n", 32));
        assert!(!sink.write_line("second line\n", 32));
        assert!(sink.write_line("third line pushes past the cap\n", 32));

        let old = std::fs::read_to_string(path.with_extension("log.old")).unwrap();
        assert!(old.starts_with("first line\n"));
        assert!(old.ends_with("third line pushes past the cap\n"));
        assert_eq!(sink.written, 0);

        assert!(!sink.write_line("fresh\n", 32));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    #[test]
    fn keeps_one_old_generation() {
        let path = temp_log("generations.log");
        let mut sink = Sink::open(path.clone()).unwrap();
        assert!(sink.write_line("generation one\n", 4));
        assert!(sink.write_line("generation two\n", 4));
        assert_eq!(
            std::fs::read_to_string(path.with_extension("log.old")).unwrap(),
            "generation two\n"
        );
    }

    #[test]
    fn counts_from_the_existing_size_at_open() {
        let path = temp_log("resume.log");
        std::fs::write(&path, "already here\n").unwrap();
        let sink = Sink::open(path.clone()).unwrap();
        assert_eq!(sink.written, "already here\n".len() as u64);
    }
}
