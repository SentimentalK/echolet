use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, VK_F10,
};

pub const HOTKEY_F10_ID: i32 = 1;

pub fn register_f10(hwnd: HWND) -> bool {
    unsafe {
        let ok = RegisterHotKey(hwnd, HOTKEY_F10_ID, MOD_NOREPEAT as u32, VK_F10 as u32);
        if ok == 0 {
            eprintln!(
                "[Warning] Failed to register global F10 hotkey (error {}). \
                 It may be in use by another application. Tray controls remain fully active.",
                GetLastError()
            );
            false
        } else {
            println!("[Platform] Registered global F10 hotkey via Win32 RegisterHotKey.");
            true
        }
    }
}

pub fn unregister_f10(hwnd: HWND) {
    unsafe {
        UnregisterHotKey(hwnd, HOTKEY_F10_ID);
    }
}
