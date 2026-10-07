use crate::platform::TextInjector;
use std::mem::size_of;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_BACK,
};

pub struct WindowsInjector;

impl WindowsInjector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInjector for WindowsInjector {
    fn apply_diff(&self, backspaces: usize, new_suffix: &str) {
        let utf16_chars: Vec<u16> = new_suffix.encode_utf16().collect();
        let total_events = (backspaces * 2) + (utf16_chars.len() * 2);
        if total_events == 0 {
            return;
        }

        let mut inputs: Vec<INPUT> = Vec::with_capacity(total_events);

        // 1. Backspaces
        for _ in 0..backspaces {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_BACK,
                        wScan: 0,
                        dwFlags: 0,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_BACK,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        // 2. Unicode characters
        for &code_unit in &utf16_chars {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0,
                        wScan: code_unit,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0,
                        wScan: code_unit,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                size_of::<INPUT>() as i32,
            );
        }
    }
}
