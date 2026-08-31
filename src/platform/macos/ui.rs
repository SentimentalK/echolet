use crate::actions::AppAction;
use crate::paths;
use crate::platform::macos::hotkey::register_global_f10;
use crate::platform::macos::injector::execute_diff;
use crate::platform::PlatformHandle;
use cocoa::appkit::{
    NSApp, NSApplication, NSApplicationActivationPolicyAccessory, NSButton,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use cocoa::base::{id, nil, selector};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use core_foundation::base::kCFAllocatorDefault;
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::runloop::{
    kCFRunLoopCommonModes, CFRunLoopAddTimer, CFRunLoopGetCurrent, CFRunLoopRef,
    CFRunLoopTimerContext, CFRunLoopTimerCreate, CFRunLoopTimerRef,
};
use crossbeam_channel::{Receiver, Sender};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub enum MacUiCommand {
    SetListening(bool),
    UpdateModels {
        active_id: String,
        installed_ids: Vec<String>,
        downloading_ids: Vec<String>,
    },
    UpdateHistoryState(bool),
    InjectDiff {
        backspaces: usize,
        suffix: String,
    },
    OpenHistoryFolder(PathBuf),
    Shutdown,
}

pub struct MacPlatformHandle {
    cmd_tx: Sender<MacUiCommand>,
}

impl MacPlatformHandle {
    pub fn new(cmd_tx: Sender<MacUiCommand>) -> Self {
        Self { cmd_tx }
    }
}

impl PlatformHandle for MacPlatformHandle {
    fn set_listening(&self, listening: bool) {
        let _ = self.cmd_tx.send(MacUiCommand::SetListening(listening));
    }

    fn shutdown(&self) {
        let _ = self.cmd_tx.send(MacUiCommand::Shutdown);
    }

    fn update_models(
        &self,
        active_id: &str,
        installed_ids: &[String],
        downloading_ids: &[String],
    ) {
        let _ = self.cmd_tx.send(MacUiCommand::UpdateModels {
            active_id: active_id.to_string(),
            installed_ids: installed_ids.to_vec(),
            downloading_ids: downloading_ids.to_vec(),
        });
    }

    fn update_history_state(&self, enabled: bool) {
        let _ = self.cmd_tx.send(MacUiCommand::UpdateHistoryState(enabled));
    }

    fn open_history_folder(&self, history_dir: &Path) {
        let _ = self
            .cmd_tx
            .send(MacUiCommand::OpenHistoryFolder(history_dir.to_path_buf()));
    }
}

static mut MENU_ACTION_TX: Option<Sender<AppAction>> = None;

extern "C" fn on_toggle_listening(_this: &Object, _cmd: Sel, _sender: id) {
    unsafe {
        if let Some(ref tx) = MENU_ACTION_TX {
            let _ = tx.send(AppAction::ToggleListening);
        }
    }
}

extern "C" fn on_toggle_history(_this: &Object, _cmd: Sel, _sender: id) {
    unsafe {
        if let Some(ref tx) = MENU_ACTION_TX {
            let _ = tx.send(AppAction::ToggleHistory);
        }
    }
}

extern "C" fn on_open_history_folder(_this: &Object, _cmd: Sel, _sender: id) {
    unsafe {
        if let Some(ref tx) = MENU_ACTION_TX {
            let _ = tx.send(AppAction::OpenHistoryFolder);
        }
    }
}

extern "C" fn on_quit(_this: &Object, _cmd: Sel, _sender: id) {
    unsafe {
        if let Some(ref tx) = MENU_ACTION_TX {
            let _ = tx.send(AppAction::Quit);
        }
    }
}

fn register_menu_delegate_class() -> &'static Class {
    static ONCE: std::sync::Once = std::sync::Once::new();
    static mut CLASS: Option<&'static Class> = None;

    ONCE.call_once(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassDecl::new("EcholetMenuDelegate", superclass).unwrap();

        unsafe {
            decl.add_method(
                sel!(onToggleListening:),
                on_toggle_listening as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(onToggleHistory:),
                on_toggle_history as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(onOpenHistoryFolder:),
                on_open_history_folder as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(onQuit:),
                on_quit as extern "C" fn(&Object, Sel, id),
            );
        }

        let registered = decl.register();
        unsafe {
            CLASS = Some(registered);
        }
    });

    unsafe { CLASS.unwrap() }
}

pub struct MacUi {
    action_tx: Sender<AppAction>,
    cmd_rx: Receiver<MacUiCommand>,
    status_item: id,
    delegate: id,
    listening: bool,
    history_enabled: bool,
    model_name: String,
    running: Arc<AtomicBool>,
}

static mut MAC_UI_PTR: *mut MacUi = std::ptr::null_mut();

extern "C" fn timer_callback(
    _timer: CFRunLoopTimerRef,
    _info: *mut c_void,
) {
    unsafe {
        if !MAC_UI_PTR.is_null() {
            (*MAC_UI_PTR).drain_commands();
        }
    }
}

impl MacUi {
    pub fn new(
        action_tx: Sender<AppAction>,
        cmd_rx: Receiver<MacUiCommand>,
    ) -> Result<Self, String> {
        unsafe {
            MENU_ACTION_TX = Some(action_tx.clone());

            let pool = NSAutoreleasePool::new(nil);
            let app = NSApp();
            app.setActivationPolicy_(NSApplicationActivationPolicyAccessory);

            let status_bar = NSStatusBar::systemStatusBar(nil);
            let status_item = status_bar.statusItemWithLength_(NSVariableStatusItemLength);

            let delegate_class = register_menu_delegate_class();
            let delegate: id = msg_send![delegate_class, new];

            let ui = Self {
                action_tx,
                cmd_rx,
                status_item,
                delegate,
                listening: false,
                history_enabled: false,
                model_name: "Chinese + English (X-ASR / 480ms)".into(),
                running: Arc::new(AtomicBool::new(true)),
            };

            let _ = NSAutoreleasePool::drain(pool);
            Ok(ui)
        }
    }

    pub fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.handle_command(cmd);
        }
    }

    pub fn handle_command(&mut self, cmd: MacUiCommand) {
        match cmd {
            MacUiCommand::SetListening(listening) => {
                self.listening = listening;
                self.update_status_bar();
                self.rebuild_menu();
            }
            MacUiCommand::UpdateHistoryState(enabled) => {
                self.history_enabled = enabled;
                self.rebuild_menu();
            }
            MacUiCommand::UpdateModels { active_id, .. } => {
                self.model_name = active_id;
                self.rebuild_menu();
            }
            MacUiCommand::InjectDiff { backspaces, suffix } => {
                execute_diff(backspaces, &suffix);
            }
            MacUiCommand::OpenHistoryFolder(path) => {
                let _ = Command::new("open").arg(&path).spawn();
            }
            MacUiCommand::Shutdown => {
                self.running.store(false, Ordering::SeqCst);
                unsafe {
                    let app = NSApp();
                    let _: () = msg_send![app, terminate:nil];
                }
            }
        }
    }

    fn update_status_bar(&self) {
        unsafe {
            let button = self.status_item.button();
            if button != nil {
                let title = if self.listening {
                    NSString::alloc(nil).init_str("●")
                } else {
                    NSString::alloc(nil).init_str("○")
                };
                button.setTitle_(title);
            }
        }
    }

    fn rebuild_menu(&self) {
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let menu = NSMenu::new(nil).autorelease();
            let _: () = msg_send![menu, setAutoenablesItems:false];

            // 1. Listening toggle row
            let toggle_label = if self.listening {
                "Stop Listening (F10)"
            } else {
                "Start Listening (F10)"
            };
            let toggle_str = NSString::alloc(nil).init_str(toggle_label);
            let key_equiv = NSString::alloc(nil).init_str("");
            let item: id = msg_send![menu, addItemWithTitle:toggle_str action:sel!(onToggleListening:) keyEquivalent:key_equiv];
            let _: () = msg_send![item, setTarget:self.delegate];
            let _: () = msg_send![item, setEnabled:true];

            // Separator
            let _: () = msg_send![menu, addItem:NSMenuItem::separatorItem(nil)];

            // 2. Model info row
            let model_label = format!("Model: {}", self.model_name);
            let model_str = NSString::alloc(nil).init_str(&model_label);
            let item: id = msg_send![menu, addItemWithTitle:model_str action:selector("none") keyEquivalent:key_equiv];
            let _: () = msg_send![item, setEnabled:false];

            // Separator
            let _: () = msg_send![menu, addItem:NSMenuItem::separatorItem(nil)];

            // 3. Local History section
            let history_path_str = paths::history_dir()
                .to_string_lossy()
                .replace(&std::env::var("HOME").unwrap_or_default(), "~");

            if !self.history_enabled {
                let hist_off_str = NSString::alloc(nil).init_str("Local History: Off");
                let item: id = msg_send![menu, addItemWithTitle:hist_off_str action:sel!(onToggleHistory:) keyEquivalent:key_equiv];
                let _: () = msg_send![item, setTarget:self.delegate];
                let _: () = msg_send![item, setEnabled:true];
            } else {
                // Enabled row
                let hist_on_str = NSString::alloc(nil).init_str("✓ Local History");
                let item: id = msg_send![menu, addItemWithTitle:hist_on_str action:sel!(onToggleHistory:) keyEquivalent:key_equiv];
                let _: () = msg_send![item, setTarget:self.delegate];
                let _: () = msg_send![item, setEnabled:true];

                // Open History Folder
                let open_str = NSString::alloc(nil).init_str("    Open History Folder");
                let item: id = msg_send![menu, addItemWithTitle:open_str action:sel!(onOpenHistoryFolder:) keyEquivalent:key_equiv];
                let _: () = msg_send![item, setTarget:self.delegate];
                let _: () = msg_send![item, setEnabled:true];

                // Path display row
                let path_label = format!("    {}", history_path_str);
                let path_str = NSString::alloc(nil).init_str(&path_label);
                let item: id = msg_send![menu, addItemWithTitle:path_str action:selector("none") keyEquivalent:key_equiv];
                let _: () = msg_send![item, setEnabled:false];
            }

            // Separator
            let _: () = msg_send![menu, addItem:NSMenuItem::separatorItem(nil)];

            // 4. Hotkey row
            let hotkey_str = NSString::alloc(nil).init_str("Hotkey: F10");
            let item: id = msg_send![menu, addItemWithTitle:hotkey_str action:selector("none") keyEquivalent:key_equiv];
            let _: () = msg_send![item, setEnabled:false];

            // 5. Quit row
            let quit_str = NSString::alloc(nil).init_str("Quit");
            let item: id = msg_send![menu, addItemWithTitle:quit_str action:sel!(onQuit:) keyEquivalent:key_equiv];
            let _: () = msg_send![item, setTarget:self.delegate];
            let _: () = msg_send![item, setEnabled:true];

            self.status_item.setMenu_(menu);
            let _ = NSAutoreleasePool::drain(pool);
        }
    }

    pub fn run(mut self) {
        unsafe {
            MAC_UI_PTR = &mut self as *mut MacUi;

            self.update_status_bar();
            self.rebuild_menu();

            // Register Carbon global F10
            let _hotkey_handle = register_global_f10(self.action_tx.clone());

            // Install CFRunLoopTimer for 15ms main thread command draining
            let mut context = CFRunLoopTimerContext {
                version: 0,
                info: std::ptr::null_mut(),
                retain: None,
                release: None,
                copyDescription: None,
            };

            let timer = CFRunLoopTimerCreate(
                kCFAllocatorDefault,
                CFAbsoluteTimeGetCurrent() + 0.015,
                0.015,
                0,
                0,
                timer_callback,
                &mut context,
            );

            let run_loop = CFRunLoopGetCurrent();
            CFRunLoopAddTimer(run_loop, timer, kCFRunLoopCommonModes);

            let app = NSApp();
            app.run();

            MAC_UI_PTR = std::ptr::null_mut();
        }
    }
}
