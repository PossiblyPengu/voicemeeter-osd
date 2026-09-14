//! Opt-in diagnostic log, so "it isn't showing" can be investigated from the
//! file rather than by instrumenting a live machine.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::SystemInformation::GetLocalTime;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Size at which the log is rotated to `debug.log.1`.
const MAX_BYTES: u64 = 1024 * 1024;

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base)
        .join("voicemeeter-osd")
        .join("debug.log")
}

pub fn write(message: &str) {
    if !enabled() {
        return;
    }
    let stamp = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    };
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep one previous log around instead of growing without bound.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{stamp}  {message}");
    }
}

/// Formats only when logging is on, so disabled logging costs nothing.
macro_rules! log_line {
    ($($arg:tt)*) => {
        if $crate::log::enabled() {
            $crate::log::write(&format!($($arg)*));
        }
    };
}
pub(crate) use log_line;
