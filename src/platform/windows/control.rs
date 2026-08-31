use crate::actions::AppAction;
use crossbeam_channel::Sender;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
    PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};

const PIPE_NAME: &str = "\\\\.\\pipe\\echolet-control";

pub struct ControlHandle {
    running: Arc<AtomicBool>,
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

pub fn start_control_listener(
    action_tx: Sender<AppAction>,
) -> Result<ControlHandle, Box<dyn std::error::Error>> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    let pipe_name_utf16: Vec<u16> = format!("{}\0", PIPE_NAME).encode_utf16().collect();

    thread::Builder::new()
        .name("echolet-win-ipc".into())
        .spawn(move || {
            while running_clone.load(Ordering::SeqCst) {
                unsafe {
                    let pipe = CreateNamedPipeW(
                        pipe_name_utf16.as_ptr(),
                        PIPE_ACCESS_DUPLEX,
                        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                        1,
                        512,
                        512,
                        50,
                        ptr::null(),
                    );

                    if pipe == INVALID_HANDLE_VALUE {
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }

                    let connected = ConnectNamedPipe(pipe, ptr::null_mut());
                    if connected != 0 || GetLastError() == 535 {
                        // ERROR_PIPE_CONNECTED
                        let mut buf = [0u8; 128];
                        let mut bytes_read = 0;
                        if ReadFile(
                            pipe,
                            buf.as_mut_ptr() as *mut _,
                            buf.len() as u32,
                            &mut bytes_read,
                            ptr::null_mut(),
                        ) != 0
                            && bytes_read > 0
                        {
                            let cmd = String::from_utf8_lossy(&buf[..bytes_read as usize]);
                            let trimmed = cmd.trim();
                            let resp = match trimmed {
                                "toggle" => {
                                    let _ = action_tx.send(AppAction::ToggleListening);
                                    "OK: toggled\n"
                                }
                                "stop" => {
                                    let _ = action_tx.send(AppAction::StopListening);
                                    "OK: stopped\n"
                                }
                                "quit" => {
                                    let _ = action_tx.send(AppAction::Quit);
                                    "OK: quitting\n"
                                }
                                "status" => "OK: running\n",
                                _ => "ERR: unknown command\n",
                            };

                            let mut bytes_written = 0;
                            WriteFile(
                                pipe,
                                resp.as_ptr() as *const _,
                                resp.len() as u32,
                                &mut bytes_written,
                                ptr::null_mut(),
                            );
                        }
                    }

                    DisconnectNamedPipe(pipe);
                    CloseHandle(pipe);
                }
            }
        })?;

    Ok(ControlHandle { running })
}

pub fn send_command(cmd: &str) -> Result<String, String> {
    let pipe_name_utf16: Vec<u16> = format!("{}\0", PIPE_NAME).encode_utf16().collect();
    unsafe {
        let handle = CreateFileW(
            pipe_name_utf16.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );

        if handle == INVALID_HANDLE_VALUE {
            return Err("Echolet daemon is not running (cannot connect to Named Pipe).".into());
        }

        let cmd_bytes = format!("{}\n", cmd);
        let mut bytes_written = 0;
        let write_ok = WriteFile(
            handle,
            cmd_bytes.as_ptr() as *const _,
            cmd_bytes.len() as u32,
            &mut bytes_written,
            ptr::null_mut(),
        );

        if write_ok == 0 {
            CloseHandle(handle);
            return Err("Failed to write command to Named Pipe.".into());
        }

        let mut buf = [0u8; 256];
        let mut bytes_read = 0;
        let read_ok = ReadFile(
            handle,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            &mut bytes_read,
            ptr::null_mut(),
        );

        CloseHandle(handle);

        if read_ok == 0 || bytes_read == 0 {
            return Err("Failed to read response from Named Pipe.".into());
        }

        Ok(String::from_utf8_lossy(&buf[..bytes_read as usize])
            .trim()
            .to_string())
    }
}
