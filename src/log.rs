//! Minimal file logger for crash diagnosis.
//!
//! Writes to `~/.echolet/echolet.log`, capped at the last `MAX_LINES` lines so
//! it never grows unbounded. Only intended for key checkpoints and errors —
//! not a replacement for every old console `println!`.
//!
//! On init this also creates `~/.echolet/` so that other subsystems (history,
//! etc.) can assume the user home dir exists.
//!
//! Captures two kinds of crashes:
//!   - Rust panics  -> via `std::panic::set_hook`
//!   - Windows native exceptions (segfault / illegal instruction, e.g. AVX2)
//!     -> via `SetUnhandledExceptionFilter`, logged with the exception code.

use crate::paths;
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Keep at most this many lines in the log file (most recent kept).
const MAX_LINES: usize = 2000;

struct LogState {
    path: PathBuf,
    buf: VecDeque<String>,
}

static LOG: Mutex<Option<LogState>> = Mutex::new(None);
/// Path stored separately (no mutex) so the native-exception handler can
/// append directly without risking a deadlock on the crashing thread.
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the logger. Must be called once at the very start of `main`,
/// before anything that might crash. Creates `~/.echolet/`, opens the log,
/// installs a panic hook, and (on Windows) a native exception filter.
pub fn init() {
    let home = paths::echolet_home_dir();
    let _ = std::fs::create_dir_all(&home);
    let log_path = home.join("echolet.log");
    let _ = LOG_PATH.set(log_path.clone());

    // Seed the ring buffer with the tail of any existing log so history
    // survives across restarts (up to MAX_LINES).
    let mut buf: VecDeque<String> = VecDeque::new();
    if let Ok(existing) = std::fs::read_to_string(&log_path) {
        for line in existing.lines() {
            buf.push_back(line.to_string());
        }
        while buf.len() > MAX_LINES {
            buf.pop_front();
        }
    }

    {
        let mut g = LOG.lock().unwrap();
        *g = Some(LogState { path: log_path, buf });
    }

    log("INFO", &format!("=== Echolet starting (pid={}) ===", std::process::id()));

    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        log("PANIC", &format!("{}\n{}", info, bt));
    }));

    #[cfg(target_os = "windows")]
    install_windows_exception_filter();
}

/// Append one line to the log. The file is rewritten from the in-memory ring
/// buffer so it stays bounded at `MAX_LINES` lines.
pub fn log(level: &str, msg: &str) {
    let line = format!("[{}] [{}] {}", ts(), level, msg);
    let mut g = LOG.lock().unwrap();
    let state = match g.as_mut() {
        Some(s) => s,
        None => return,
    };
    state.buf.push_back(line.clone());
    if state.buf.len() > MAX_LINES {
        state.buf.pop_front();
    }
    if let Ok(f) = File::create(&state.path) {
        let mut f = f;
        for l in &state.buf {
            let _ = writeln!(f, "{}", l);
        }
        let _ = f.flush();
    }
}

/// Append a line bypassing the mutex — used only from the native-exception
/// handler, which may run on a thread that already holds `LOG`.
fn log_crash_direct(line: &str) {
    if let Some(path) = LOG_PATH.get() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{}", line);
            let _ = f.flush();
        }
    }
}

fn ts() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ---------------------------------------------------------------------------
// Windows native exception filter
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod win_exc {
    use super::log_crash_direct;
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};

    // EXCEPTION_CONTINUE_SEARCH — log, then let the default crash handling
    // (Windows Error Reporting / minidump) proceed so the process still exits.
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    unsafe extern "system" fn handler(info: *const EXCEPTION_POINTERS) -> i32 {
        if !info.is_null() {
            let rec_ptr = (*info).ExceptionRecord;
            if !rec_ptr.is_null() {
                let rec = &*rec_ptr;
                let code = rec.ExceptionCode as u32;
                let addr = rec.ExceptionAddress as usize;
                let label = match code {
                    0xC0000005 => "ACCESS_VIOLATION",
                    0xC000001D => "ILLEGAL_INSTRUCTION (likely missing AVX2 on old CPU)",
                    0xC0000094 => "PRIV_INSTRUCTION",
                    0xC00000FD => "STACK_OVERFLOW",
                    0xC0000017 => "NO_MEMORY",
                    _ => "EXCEPTION",
                };
                log_crash_direct(&format!(
                    "[{}] [FATAL] native {} code=0x{:08X} addr=0x{:016X}",
                    super::ts(),
                    label,
                    code,
                    addr
                ));
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    pub fn install() {
        unsafe {
            SetUnhandledExceptionFilter(Some(handler));
        }
    }
}

#[cfg(target_os = "windows")]
fn install_windows_exception_filter() {
    win_exc::install();
    log("INFO", "Windows native exception filter installed");
}
