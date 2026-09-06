use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Directory holding every file the daemon persists: portable (next to the
/// exe) when a `settings.json` already sits there, otherwise
/// `%LOCALAPPDATA%\WinSpaces`.
pub fn config_dir() -> PathBuf {
    if let Ok(mut exe_dir) = std::env::current_exe() {
        exe_dir.pop();
        if exe_dir.join("settings.json").exists() {
            return exe_dir;
        }
    }

    if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
        let dir = PathBuf::from(appdata).join("WinSpaces");
        let _ = fs::create_dir_all(&dir);
        return dir;
    }

    PathBuf::from(".")
}

/// Serialize to a sibling temp file, then rename over the target. `fs::write`
/// truncates first, so a crash mid-write leaves a half-written file behind;
/// rename is atomic on NTFS, so a reader sees either the old file or the new
/// one and never a truncated one.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = temp_path(path);
    fs::write(&tmp, json)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// `settings.json` -> `settings.json.<pid>.tmp`. The daemon and the settings
/// window are separate processes writing the same file; a shared temp name
/// would let one of them rename the other's half-written file into place.
fn temp_path(path: &Path) -> PathBuf {
    path.with_extension(format!("json.{}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_through_a_temp_file_and_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("winspaces-paths-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("settings.json");
        write_json_atomic(&path, &vec![1, 2, 3]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "[\n  1,\n  2,\n  3\n]");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("settings.json")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_name_is_per_process() {
        let tmp = temp_path(Path::new(r"C:\WinSpaces\settings.json"));
        assert!(tmp
            .to_string_lossy()
            .ends_with(&format!(r"\settings.json.{}.tmp", std::process::id())));
    }
}
