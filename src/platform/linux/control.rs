use crate::actions::AppAction;
use crossbeam_channel::Sender;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("voice_input_toggle.sock")
    } else {
        PathBuf::from("/tmp/voice_input_toggle.sock")
    }
}

pub fn send_toggle_signal() -> Result<(), String> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("Failed to connect to daemon socket at {:?}: {}", socket_path, e))?;
    stream
        .write_all(b"TOGGLE\n")
        .map_err(|e| format!("Failed to write to socket: {}", e))?;
    println!("[Client] Sent toggle command to running instance.");
    Ok(())
}

pub fn send_stop_signal() -> Result<(), String> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("Echolet daemon is not running (cannot connect to {:?}): {}", socket_path, e))?;
    stream
        .write_all(b"QUIT\n")
        .map_err(|e| format!("Failed to write to socket: {}", e))?;
    println!("[Client] Sent stop command to running instance. Exiting.");
    Ok(())
}

pub fn send_status_signal() -> Result<(), String> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|_| format!("Echolet is NOT running (no active socket at {:?})", socket_path))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
    stream
        .write_all(b"STATUS\n")
        .map_err(|e| format!("Failed to write to socket: {}", e))?;
    let mut buf = [0u8; 64];
    let n = stream
        .read(&mut buf)
        .map_err(|e| format!("Failed to read response: {}", e))?;
    let reply = String::from_utf8_lossy(&buf[..n]);
    println!("[Status] {}", reply.trim());
    Ok(())
}

pub struct ControlHandle {
    running: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
    socket_path: PathBuf,
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if self.socket_path.exists() {
            let _ = fs::remove_file(&self.socket_path);
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Starts the Unix domain socket control listener.
/// Handles single-instance protection:
/// - If responsive instance exists -> returns Ok(None) to signal immediate clean exit.
/// - If stale socket exists -> cleans up socket and binds.
pub fn start_control_listener(
    action_tx: Sender<AppAction>,
) -> Result<Option<ControlHandle>, Box<dyn std::error::Error>> {
    let socket_path = get_socket_path();

    if socket_path.exists() {
        match UnixStream::connect(&socket_path) {
            Ok(_) => {
                println!("[SingleInstance] Another instance of Echolet is already running. Exiting.");
                return Ok(None);
            }
            Err(_) => {
                println!("[Control] Cleaning up stale socket at {:?}", socket_path);
                let _ = fs::remove_file(&socket_path);
            }
        }
    }

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    println!("[Control] Listening for toggle events on {:?}", socket_path);

    let running = Arc::new(AtomicBool::new(true));
    let r_clone = running.clone();

    let thread_handle = thread::spawn(move || {
        while r_clone.load(Ordering::SeqCst) {
            if let Ok((mut client, _)) = listener.accept() {
                let _ = client.set_nonblocking(false);
                let _ = client.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 64];
                if let Ok(n) = client.read(&mut buf) {
                    if n > 0 {
                        let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_uppercase();
                        match cmd.as_str() {
                            "TOGGLE" => {
                                let _ = action_tx.send(AppAction::ToggleListening);
                            }
                            "QUIT" | "STOP" => {
                                let _ = action_tx.send(AppAction::Quit);
                            }
                            "STATUS" => {
                                let _ = client.write_all(b"Echolet is running in background.\n");
                            }
                            _ => {}
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    });

    Ok(Some(ControlHandle {
        running,
        thread_handle: Some(thread_handle),
        socket_path,
    }))
}
