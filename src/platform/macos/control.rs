use crate::actions::AppAction;
use crate::paths;
use crossbeam_channel::Sender;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn socket_path() -> PathBuf {
    paths::echolet_home_dir().join("echolet.sock")
}

pub struct ControlHandle {
    running: Arc<AtomicBool>,
    socket_path: PathBuf,
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub fn start_control_listener(
    action_tx: Sender<AppAction>,
) -> Result<Option<ControlHandle>, Box<dyn std::error::Error>> {
    let sock = socket_path();
    let _ = fs::create_dir_all(paths::echolet_home_dir());

    if sock.exists() {
        if let Ok(mut stream) = UnixStream::connect(&sock) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let _ = stream.write_all(b"status\n");
            let mut buf = [0u8; 64];
            if let Ok(n) = stream.read(&mut buf) {
                if n > 0 {
                    println!("[Echolet] Another instance is already running.");
                    return Ok(None);
                }
            }
        }
        let _ = fs::remove_file(&sock);
    }

    let listener = UnixListener::bind(&sock)?;
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let sock_clone = sock.clone();

    thread::Builder::new()
        .name("echolet-mac-ipc".into())
        .spawn(move || {
            let _ = listener.set_nonblocking(true);
            while running_clone.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 128];
                        if let Ok(n) = stream.read(&mut buf) {
                            let cmd = String::from_utf8_lossy(&buf[..n]);
                            let trimmed = cmd.trim();
                            match trimmed {
                                "toggle" => {
                                    let _ = action_tx.send(AppAction::ToggleListening);
                                    let _ = stream.write_all(b"OK: toggled\n");
                                }
                                "stop" => {
                                    let _ = action_tx.send(AppAction::StopListening);
                                    let _ = stream.write_all(b"OK: stopped\n");
                                }
                                "quit" => {
                                    let _ = action_tx.send(AppAction::Quit);
                                    let _ = stream.write_all(b"OK: quitting\n");
                                }
                                "status" => {
                                    let _ = stream.write_all(b"OK: running\n");
                                }
                                _ => {
                                    let _ = stream.write_all(b"ERR: unknown command\n");
                                }
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })?;

    Ok(Some(ControlHandle {
        running,
        socket_path: sock,
    }))
}

pub fn send_command(cmd: &str) -> Result<String, String> {
    let sock = socket_path();
    let mut stream = UnixStream::connect(&sock)
        .map_err(|e| format!("Echolet daemon is not running (cannot connect to {:?}): {}", sock, e))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;

    stream
        .write_all(format!("{}\n", cmd).as_bytes())
        .map_err(|e| e.to_string())?;

    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}
