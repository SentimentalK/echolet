#![allow(deprecated)]

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
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

type CGEventSourceRef = *mut c_void;
type CGEventRef = *mut c_void;
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
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attribute: *const c_void,
        value: *mut *const c_void,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: *mut c_void,
        attribute: *const c_void,
        value: *const c_void,
    ) -> i32;
    fn AXValueCreate(value_type: u32, value: *const c_void) -> *const c_void;
    fn AXValueGetValue(value: *const c_void, value_type: u32, value_out: *mut c_void) -> bool;
}

const K_AX_VALUE_CF_RANGE: u32 = 4;
const NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS: usize = 2;

#[repr(C)]
struct CFRange {
    location: isize,
    length: isize,
}

static HAS_CHECKED_ACCESSIBILITY: AtomicBool = AtomicBool::new(false);
static LAST_EXTERNAL_PID: AtomicI32 = AtomicI32::new(0);
static LOGGED_INJECT_OK: AtomicBool = AtomicBool::new(false);
static LOGGED_INJECT_BLOCKED: AtomicBool = AtomicBool::new(false);

fn log_inject_once(slot: &AtomicBool, level: &str, msg: &str) {
    if !slot.swap(true, Ordering::Relaxed) {
        crate::log::log(level, msg);
    }
}

/// Remember the frontmost app that is not Echolet, so dictated text can be
/// inserted there even while the floating panel is key.
pub fn remember_frontmost_app() {
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace == nil {
            return;
        }
        let app: id = msg_send![workspace, frontmostApplication];
        if app == nil {
            return;
        }
        let pid: i32 = msg_send![app, processIdentifier];
        if pid > 0 && pid != std::process::id() as i32 {
            LAST_EXTERNAL_PID.store(pid, Ordering::Relaxed);
        }
    }
}

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

/// Logs and opens Accessibility settings when this build cannot type.
///
/// Ad-hoc signatures change on every rebuild, so a previously enabled
/// Echolet entry can still fail `AXIsProcessTrusted`.
pub fn warn_if_accessibility_missing() {
    if is_accessibility_trusted() {
        return;
    }

    let msg = "Accessibility permission is missing for this build. Speech is heard, but keystrokes are dropped until Echolet is enabled in System Settings → Privacy & Security → Accessibility. If it is already listed, turn it off and on.";
    crate::log::log("WARN", msg);
    eprintln!("[Accessibility] {}", msg);
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

fn ax_attr(name: &str) -> CFString {
    CFString::new(name)
}

/// Insert `suffix` into the last non-Echolet app's focused text field.
/// Returns false when that field cannot be edited, before any text is changed.
fn insert_into_last_app(backspaces: usize, suffix: &str) -> bool {
    let pid = LAST_EXTERNAL_PID.load(Ordering::Relaxed);
    if pid <= 0 {
        log_inject_once(
            &LOGGED_INJECT_BLOCKED,
            "WARN",
            "inject skipped: click a text field before turning voice on",
        );
        return false;
    }

    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        let focused_attr = ax_attr("AXFocusedUIElement");
        let mut focused: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(
            app,
            focused_attr.as_concrete_TypeRef() as *const c_void,
            &mut focused,
        );
        if err != 0 || focused.is_null() {
            CFRelease(app);
            log_inject_once(
                &LOGGED_INJECT_BLOCKED,
                "WARN",
                &format!("inject skipped: no focused text field in pid {pid} (AX {err})"),
            );
            return false;
        }

        if backspaces > 0 && !select_backspaces(focused, backspaces) {
            CFRelease(focused);
            CFRelease(app);
            return false;
        }

        let text_attr = ax_attr("AXSelectedText");
        let text = CFString::new(suffix);
        let err = AXUIElementSetAttributeValue(
            focused as *mut c_void,
            text_attr.as_concrete_TypeRef() as *const c_void,
            text.as_concrete_TypeRef() as *const c_void,
        );
        CFRelease(focused);
        CFRelease(app);
        if err != 0 {
            log_inject_once(
                &LOGGED_INJECT_BLOCKED,
                "WARN",
                &format!("inject skipped: text field rejected the update (AX {err})"),
            );
            return false;
        }
        true
    }
}

fn select_backspaces(element: *const c_void, backspaces: usize) -> bool {
    unsafe {
        let range_attr = ax_attr("AXSelectedTextRange");
        let mut range_value: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(
            element as *mut c_void,
            range_attr.as_concrete_TypeRef() as *const c_void,
            &mut range_value,
        );
        if err != 0 || range_value.is_null() {
            return false;
        }
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        let ok = AXValueGetValue(
            range_value,
            K_AX_VALUE_CF_RANGE,
            &mut range as *mut CFRange as *mut c_void,
        );
        CFRelease(range_value);
        if !ok {
            return false;
        }
        let bs = backspaces as isize;
        if range.location < bs {
            return false;
        }
        range.location -= bs;
        range.length += bs;
        let new_range = AXValueCreate(
            K_AX_VALUE_CF_RANGE,
            &range as *const CFRange as *const c_void,
        );
        if new_range.is_null() {
            return false;
        }
        let err = AXUIElementSetAttributeValue(
            element as *mut c_void,
            range_attr.as_concrete_TypeRef() as *const c_void,
            new_range,
        );
        CFRelease(new_range);
        err == 0
    }
}

fn activate_last_app() {
    let pid = LAST_EXTERNAL_PID.load(Ordering::Relaxed);
    if pid <= 0 {
        return;
    }
    unsafe {
        let app: id = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if app != nil {
            let _: bool = msg_send![app, activateWithOptions: NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS];
        }
    }
}

/// Executes Backspaces and text paste on the main thread.
pub fn execute_diff(backspaces: usize, suffix: &str) {
    if backspaces == 0 && suffix.is_empty() {
        return;
    }
    if !is_accessibility_trusted() {
        log_inject_once(
            &LOGGED_INJECT_BLOCKED,
            "WARN",
            "inject skipped: Accessibility permission is still missing for this build",
        );
        eprintln!(
            "[Accessibility] Warning: Echolet requires Accessibility permissions to inject text.\n\
             Please enable Echolet in System Settings -> Privacy & Security -> Accessibility."
        );
        return;
    }

    if insert_into_last_app(backspaces, suffix) {
        log_inject_once(
            &LOGGED_INJECT_OK,
            "INFO",
            &format!(
                "inject ok pid={} suffix={:?}",
                LAST_EXTERNAL_PID.load(Ordering::Relaxed),
                suffix
            ),
        );
        return;
    }

    activate_last_app();
    if backspaces > 0 || !suffix.is_empty() {
        thread::sleep(Duration::from_millis(30));
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
                CGEventSetFlags(up, K_CG_EVENT_FLAG_MASK_COMMAND);
                CGEventPost(K_CG_HID_EVENT_TAP, down);
                CGEventPost(K_CG_HID_EVENT_TAP, up);
                CFRelease(down as *const c_void);
                CFRelease(up as *const c_void);
            }
        }
    }
}
