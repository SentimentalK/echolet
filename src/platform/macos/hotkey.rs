use crate::actions::AppAction;
use crossbeam_channel::Sender;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

type EventTargetRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type OSStatus = i32;

const K_EVENT_CLASS_KEYBOARD: u32 = 0x6b657962; // 'keyb'
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_VK_F10: u32 = 109; // 0x6D

extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn RegisterEventHotKey(
        in_hot_key_code: u32,
        in_hot_key_modifiers: u32,
        in_hot_key_id: EventHotKeyID,
        in_target: EventTargetRef,
        in_options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(in_hot_key_ref: EventHotKeyRef) -> OSStatus;
    fn InstallEventHandler(
        in_target: EventTargetRef,
        in_handler: extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus,
        in_num_types: u32,
        in_list: *const EventTypeSpec,
        in_user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RemoveEventHandler(in_handler_ref: EventHandlerRef) -> OSStatus;
}

static mut GLOBAL_ACTION_TX: Option<Sender<AppAction>> = None;

extern "C" fn hotkey_handler_callback(
    _call_ref: EventHandlerCallRef,
    _event_ref: EventRef,
    _user_data: *mut c_void,
) -> OSStatus {
    unsafe {
        if let Some(ref tx) = GLOBAL_ACTION_TX {
            let _ = tx.send(AppAction::ToggleListening);
        }
    }
    0 // noErr
}

pub struct MacHotkeyHandle {
    hotkey_ref: EventHotKeyRef,
    handler_ref: EventHandlerRef,
    _active: Arc<AtomicBool>,
}

impl Drop for MacHotkeyHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.hotkey_ref.is_null() {
                UnregisterEventHotKey(self.hotkey_ref);
            }
            if !self.handler_ref.is_null() {
                RemoveEventHandler(self.handler_ref);
            }
            GLOBAL_ACTION_TX = None;
        }
    }
}

pub fn register_global_f10(action_tx: Sender<AppAction>) -> Result<MacHotkeyHandle, String> {
    unsafe {
        GLOBAL_ACTION_TX = Some(action_tx);

        let event_target = GetApplicationEventTarget();
        if event_target.is_null() {
            return Err("Failed to get application event target".into());
        }

        let event_spec = EventTypeSpec {
            event_class: K_EVENT_CLASS_KEYBOARD,
            event_kind: K_EVENT_HOT_KEY_PRESSED,
        };

        let mut handler_ref: EventHandlerRef = std::ptr::null_mut();
        let status = InstallEventHandler(
            event_target,
            hotkey_handler_callback,
            1,
            &event_spec,
            std::ptr::null_mut(),
            &mut handler_ref,
        );

        if status != 0 {
            return Err(format!("Failed to install Carbon event handler: {}", status));
        }

        let hotkey_id = EventHotKeyID {
            signature: 0x4543484F, // 'ECHO'
            id: 1,
        };

        let mut hotkey_ref: EventHotKeyRef = std::ptr::null_mut();
        let status = RegisterEventHotKey(
            K_VK_F10,
            0, // No modifier, standalone F10
            hotkey_id,
            event_target,
            0,
            &mut hotkey_ref,
        );

        if status != 0 {
            let _ = RemoveEventHandler(handler_ref);
            return Err(format!("Failed to register Carbon F10 hotkey: {}", status));
        }

        println!("[Platform] Registered global F10 hotkey via Carbon API.");

        Ok(MacHotkeyHandle {
            hotkey_ref,
            handler_ref,
            _active: Arc::new(AtomicBool::new(true)),
        })
    }
}
