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

pub fn start_control_listener(
    action_tx: Sender<AppAction>,
) -> Result<ControlHandle, Box<dyn std::error::Error>> {
    let socket_path = get_socket_path();
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    println!("[Control] Listening for toggle events on {:?}", socket_path);

    let running = Arc::new(AtomicBool::new(true));
    let r_clone = running.clone();

    let thread_handle = thread::spawn(move || {
        while r_clone.load(Ordering::SeqCst) {
            if let Ok((mut client, _)) = listener.accept() {
                let mut buf = [0u8; 16];
                let _ = client.read(&mut buf);
                let _ = action_tx.send(AppAction::ToggleListening);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });

    Ok(ControlHandle {
        running,
        thread_handle: Some(thread_handle),
        socket_path,
    })
}
