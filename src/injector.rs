use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use arboard::Clipboard;

const UI_SET_EVBIT: libc::c_ulong = 0x40045564;
const UI_SET_KEYBIT: libc::c_ulong = 0x40045565;
const UI_DEV_SETUP: libc::c_ulong = 0x405c5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const SYN_REPORT: u16 = 0x00;

const KEY_BACKSPACE: u16 = 14;
const KEY_LEFTCTRL: u16 = 29;
const KEY_V: u16 = 47;

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

pub struct WaylandInjector {
    uinput_file: Option<File>,
    clipboard: Mutex<Option<Clipboard>>,
}

impl WaylandInjector {
    pub fn new() -> Self {
        let uinput_file = Self::setup_uinput();
        if uinput_file.is_some() {
            println!("[Injector] /dev/uinput virtual keyboard initialized successfully.");
        } else {
            eprintln!("[Injector] Warning: Could not initialize /dev/uinput virtual keyboard.");
        }

        let clipboard = match Clipboard::new() {
            Ok(cb) => {
                println!("[Injector] Clipboard initialized successfully.");
                Mutex::new(Some(cb))
            }
            Err(e) => {
                eprintln!("[Injector] Warning: Failed to initialize clipboard: {}", e);
                Mutex::new(None)
            }
        };

        Self {
            uinput_file,
            clipboard,
        }
    }

    fn setup_uinput() -> Option<File> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")
            .ok()?;

        let fd = file.as_raw_fd();

        unsafe {
            if libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_int) < 0 {
                return None;
            }
            if libc::ioctl(fd, UI_SET_EVBIT, EV_SYN as libc::c_int) < 0 {
                return None;
            }

            // Enable keys
            for &key in &[KEY_BACKSPACE, KEY_LEFTCTRL, KEY_V] {
                if libc::ioctl(fd, UI_SET_KEYBIT, key as libc::c_int) < 0 {
                    return None;
                }
            }

            let mut setup: UinputSetup = std::mem::zeroed();
            setup.id.bustype = 0x03; // BUS_USB
            setup.id.vendor = 0x1234;
            setup.id.product = 0x5678;
            setup.id.version = 1;

            let dev_name = b"Voice Assistant Keyboard\0";
            setup.name[..dev_name.len()].copy_from_slice(dev_name);

            if libc::ioctl(fd, UI_DEV_SETUP, &setup) < 0 {
                return None;
            }
            if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
                return None;
            }
        }

        // Wait brief moment for OS/compositor to enumerate device
        thread::sleep(Duration::from_millis(100));

        Some(file)
    }

    fn emit_event(fd: RawFd, type_: u16, code: u16, value: i32) {
        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_,
            code,
            value,
        };

        unsafe {
            libc::write(
                fd,
                &event as *const InputEvent as *const libc::c_void,
                std::mem::size_of::<InputEvent>(),
            );
        }
    }

    fn sync_events(fd: RawFd) {
        Self::emit_event(fd, EV_SYN, SYN_REPORT, 0);
    }

    pub fn send_backspaces(&self, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(file) = &self.uinput_file {
            let fd = file.as_raw_fd();
            for _ in 0..count {
                Self::emit_event(fd, EV_KEY, KEY_BACKSPACE, 1);
                Self::sync_events(fd);
                Self::emit_event(fd, EV_KEY, KEY_BACKSPACE, 0);
                Self::sync_events(fd);
                thread::sleep(Duration::from_micros(500));
            }
        }
    }

    pub fn paste_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }

        // 1. Put text into clipboard
        if let Ok(mut cb_guard) = self.clipboard.lock() {
            if let Some(cb) = cb_guard.as_mut() {
                let _ = cb.set_text(text);
            }
        }

        // 2. Press Ctrl+V via uinput virtual keyboard
        if let Some(file) = &self.uinput_file {
            let fd = file.as_raw_fd();

            // Press Ctrl
            Self::emit_event(fd, EV_KEY, KEY_LEFTCTRL, 1);
            Self::sync_events(fd);

            // Press V
            Self::emit_event(fd, EV_KEY, KEY_V, 1);
            Self::sync_events(fd);

            // Release V
            Self::emit_event(fd, EV_KEY, KEY_V, 0);
            Self::sync_events(fd);

            // Release Ctrl
            Self::emit_event(fd, EV_KEY, KEY_LEFTCTRL, 0);
            Self::sync_events(fd);
        }
    }

    pub fn apply_diff(&self, backspaces: usize, new_suffix: &str) {
        if backspaces > 0 {
            self.send_backspaces(backspaces);
        }
        if !new_suffix.is_empty() {
            self.paste_text(new_suffix);
        }
    }
}

impl Drop for WaylandInjector {
    fn drop(&mut self) {
        if let Some(file) = &self.uinput_file {
            let fd = file.as_raw_fd();
            unsafe {
                libc::ioctl(fd, UI_DEV_DESTROY);
            }
        }
    }
}
