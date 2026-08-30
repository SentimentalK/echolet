use crate::platform::TextInjector;
use arboard::Clipboard;
use libc::{c_int, c_uint, ioctl, suseconds_t, time_t, timeval};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

// Linux input event definitions
const EV_KEY: u16 = 0x01;
const EV_SYN: u16 = 0x00;
const SYN_REPORT: u16 = 0;

const KEY_BACKSPACE: u16 = 14;
const KEY_LEFTCTRL: u16 = 29;
const KEY_V: u16 = 47;

const UI_SET_EVBIT: c_uint = 0x40045564;
const UI_SET_KEYBIT: c_uint = 0x40045565;
const UI_DEV_CREATE: c_uint = 0x5501;
const UI_DEV_DESTROY: c_uint = 0x5502;

#[repr(C)]
struct InputEvent {
    time: timeval,
    type_: u16,
    code: u16,
    value: i32,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

pub struct LinuxInjector {
    uinput_file: Mutex<Option<File>>,
    clipboard: Mutex<Option<Clipboard>>,
}

impl LinuxInjector {
    pub fn new() -> Self {
        let uinput_file = match Self::init_uinput() {
            Ok(file) => {
                println!("[Injector] /dev/uinput virtual keyboard initialized successfully.");
                Some(file)
            }
            Err(e) => {
                eprintln!("[Injector] Warning: /dev/uinput could not be initialized: {}. Backspace keystrokes disabled.", e);
                None
            }
        };

        let clipboard = match Clipboard::new() {
            Ok(cb) => {
                println!("[Injector] Clipboard initialized successfully.");
                Some(cb)
            }
            Err(e) => {
                eprintln!("[Injector] Warning: Failed to initialize clipboard: {}. Fast paste disabled.", e);
                None
            }
        };

        Self {
            uinput_file: Mutex::new(uinput_file),
            clipboard: Mutex::new(clipboard),
        }
    }

    fn init_uinput() -> Result<File, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")
            .map_err(|e| format!("Failed to open /dev/uinput: {}", e))?;

        let fd = file.as_raw_fd();

        unsafe {
            if ioctl(fd, UI_SET_EVBIT as _, EV_KEY as c_int) < 0 {
                return Err("ioctl UI_SET_EVBIT EV_KEY failed".into());
            }
            if ioctl(fd, UI_SET_EVBIT as _, EV_SYN as c_int) < 0 {
                return Err("ioctl UI_SET_EVBIT EV_SYN failed".into());
            }

            for &key in &[KEY_BACKSPACE, KEY_LEFTCTRL, KEY_V] {
                if ioctl(fd, UI_SET_KEYBIT as _, key as c_int) < 0 {
                    return Err(format!("ioctl UI_SET_KEYBIT key {} failed", key));
                }
            }

            let mut setup: UinputSetup = std::mem::zeroed();
            setup.id.bustype = 0x03; // BUS_USB
            setup.id.vendor = 0x1234;
            setup.id.product = 0x5678;
            setup.id.version = 1;

            let name = CString::new("Echolet Virtual Keyboard").unwrap();
            let name_bytes = name.as_bytes_with_nul();
            setup.name[..name_bytes.len()].copy_from_slice(name_bytes);

            const UI_DEV_SETUP: c_uint = 0x405c5503;
            if ioctl(fd, UI_DEV_SETUP as _, &setup) < 0 {
                return Err("ioctl UI_DEV_SETUP failed".into());
            }

            if ioctl(fd, UI_DEV_CREATE as _) < 0 {
                return Err("ioctl UI_DEV_CREATE failed".into());
            }
        }

        // Give the OS a moment to register the new virtual input device
        thread::sleep(Duration::from_millis(150));
        Ok(file)
    }

    fn emit_event(file: &mut File, type_: u16, code: u16, value: i32) {
        let ev = InputEvent {
            time: timeval {
                tv_sec: 0 as time_t,
                tv_usec: 0 as suseconds_t,
            },
            type_,
            code,
            value,
        };

        let ev_slice = unsafe {
            std::slice::from_raw_parts(
                &ev as *const _ as *const u8,
                std::mem::size_of::<InputEvent>(),
            )
        };
        use std::io::Write;
        let _ = file.write_all(ev_slice);
    }

    fn emit_syn(file: &mut File) {
        Self::emit_event(file, EV_SYN, SYN_REPORT, 0);
    }

    fn emit_key_click(file: &mut File, code: u16) {
        Self::emit_event(file, EV_KEY, code, 1);
        Self::emit_syn(file);
        thread::sleep(Duration::from_micros(500));
        Self::emit_event(file, EV_KEY, code, 0);
        Self::emit_syn(file);
    }

    fn emit_ctrl_v(file: &mut File) {
        Self::emit_event(file, EV_KEY, KEY_LEFTCTRL, 1);
        Self::emit_syn(file);
        thread::sleep(Duration::from_millis(1));

        Self::emit_event(file, EV_KEY, KEY_V, 1);
        Self::emit_syn(file);
        thread::sleep(Duration::from_millis(1));

        Self::emit_event(file, EV_KEY, KEY_V, 0);
        Self::emit_syn(file);
        thread::sleep(Duration::from_millis(1));

        Self::emit_event(file, EV_KEY, KEY_LEFTCTRL, 0);
        Self::emit_syn(file);
    }
}

impl Drop for LinuxInjector {
    fn drop(&mut self) {
        if let Ok(mut lock) = self.uinput_file.lock() {
            if let Some(file) = lock.take() {
                unsafe {
                    ioctl(file.as_raw_fd(), UI_DEV_DESTROY as _);
                }
            }
        }
    }
}

impl TextInjector for LinuxInjector {
    fn apply_diff(&self, backspaces: usize, new_suffix: &str) {
        let mut uinput_lock = self.uinput_file.lock().unwrap();

        // 1. Emit required backspaces
        if backspaces > 0 {
            if let Some(file) = uinput_lock.as_mut() {
                for _ in 0..backspaces {
                    Self::emit_key_click(file, KEY_BACKSPACE);
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }

        // 2. Paste new suffix using clipboard + Ctrl+V
        if !new_suffix.is_empty() {
            if let Ok(mut cb_lock) = self.clipboard.lock() {
                if let Some(cb) = cb_lock.as_mut() {
                    let _ = cb.set_text(new_suffix);

                    if let Some(file) = uinput_lock.as_mut() {
                        thread::sleep(Duration::from_millis(5));
                        Self::emit_ctrl_v(file);
                    }
                }
            }
        }
    }
}
