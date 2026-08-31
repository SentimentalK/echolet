use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

pub struct SingleInstanceGuard {
    handle: HANDLE,
}

unsafe impl Send for SingleInstanceGuard {}
unsafe impl Sync for SingleInstanceGuard {}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

pub fn acquire_single_instance() -> Result<Option<SingleInstanceGuard>, String> {
    let mutex_name: Vec<u16> = "Local\\EcholetDesktopSingleton\0"
        .encode_utf16()
        .collect();
    unsafe {
        let handle = CreateMutexW(ptr::null(), 1, mutex_name.as_ptr());
        if handle == 0 {
            return Err(format!("Failed to create mutex: {}", GetLastError()));
        }

        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return Ok(None);
        }

        Ok(Some(SingleInstanceGuard { handle }))
    }
}
