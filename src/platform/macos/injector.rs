use crate::platform::macos::ui::MacUiCommand;
use crate::platform::TextInjector;
use cocoa::base::{id, nil};
use cocoa::foundation::NSString;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use crossbeam_channel::Sender;
use objc::{class, msg_send, sel, sel_impl};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[repr(C)]
struct __CGEventSource;
type CGEventSourceRef = *mut __CGEventSource;

#[repr(C)]
struct __CGEvent;
type CGEventRef = *mut __CGEvent;

type CGKeyCode = u16;
type CGEventTapLocation = u32;
type CGEventFlags = u64;

const K_CG_HID_EVENT_TAP: CGEventTapLocation = 0;
const K_CG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x00100000;
const K_VK_DELETE: CGKeyCode = 0x33; // Backspace
const K_VK_ANSI_V: CGKeyCode = 0x09; // 'V'

extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        virtual_key: CGKeyCode,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
    fn CGEventPost(tap: CGEventTapLocation, event: CGEventRef);
    fn CFRelease(cf: *const c_void);
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

static HAS_CHECKED_ACCESSIBILITY: AtomicBool = AtomicBool::new(false);

pub struct MacInjector {
    cmd_tx: Sender<MacUiCommand>,
}

impl MacInjector {
    pub fn new(cmd_tx: Sender<MacUiCommand>) -> Self {
        Self { cmd_tx }
    }
}

impl TextInjector for MacInjector {
    fn apply_diff(&self, backspaces: usize, new_suffix: &str) {
        let _ = self.cmd_tx.send(MacUiCommand::InjectDiff {
            backspaces,
            suffix: new_suffix.to_string(),
        });
    }
}

/// Checks accessibility trust and prompts user if not trusted yet.
pub fn is_accessibility_trusted() -> bool {
    let check_prompt = !HAS_CHECKED_ACCESSIBILITY.swap(true, Ordering::SeqCst);
    let prompt_key = CFString::new("AXTrustedCheckOptionPrompt");
    let prompt_val = if check_prompt {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };

    let dict = CFDictionary::from_CFType_pairs(&[(prompt_key, prompt_val)]);
    unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef() as *const c_void) }
}

/// Executes Backspaces and text paste on the main thread.
pub fn execute_diff(backspaces: usize, suffix: &str) {
    if !is_accessibility_trusted() {
        eprintln!(
            "[Accessibility] Warning: Echolet requires Accessibility permissions to inject text.\n\
             Please enable Echolet in System Settings -> Privacy & Security -> Accessibility."
        );
        return;
    }

    unsafe {
        // 1. Send Backspace events
        for _ in 0..backspaces {
            let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), K_VK_DELETE, true);
            let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), K_VK_DELETE, false);
            if !down.is_null() && !up.is_null() {
                CGEventPost(K_CG_HID_EVENT_TAP, down);
                CGEventPost(K_CG_HID_EVENT_TAP, up);
                CFRelease(down as *const c_void);
                CFRelease(up as *const c_void);
            }
        }

        // 2. If suffix is not empty, copy to NSPasteboard and simulate Command+V
        if !suffix.is_empty() {
            let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard != nil {
                let _: () = msg_send![pasteboard, clearContents];
                let ns_str = NSString::alloc(nil).init_str(suffix);
                let ns_type = NSString::alloc(nil).init_str("public.utf8-plain-text");
                let _: () = msg_send![pasteboard, setString:ns_str forType:ns_type];
            }

            if backspaces > 0 {
                thread::sleep(Duration::from_millis(5));
            }

            // Simulate Command + V
            let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), K_VK_ANSI_V, true);
            let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), K_VK_ANSI_V, false);
            if !down.is_null() && !up.is_null() {
                CGEventSetFlags(down, K_CG_EVENT_FLAG_MASK_COMMAND);
                CGEventSetFlags(up, 0);
                CGEventPost(K_CG_HID_EVENT_TAP, down);
                CGEventPost(K_CG_HID_EVENT_TAP, up);
                CFRelease(down as *const c_void);
                CFRelease(up as *const c_void);
            }
        }
    }
}
