#![allow(deprecated)]

use crate::actions::AppAction;
use crate::paths;
use crate::platform::macos::hotkey::register_global_f10;
use crate::platform::macos::injector::execute_diff;
use crate::platform::{PlatformHandle, PlatformView};
use crate::ui::control_surface::ControlSurfaceState;
use crate::ui::desktop::host::{calculate_macos_panel_position, Rect};
use crate::ui::desktop::{DesktopPanelController, PANEL_HEIGHT_PX, PANEL_WIDTH_PX};
use cocoa::appkit::{
    NSApp, NSApplication, NSApplicationActivationPolicyAccessory, NSButton, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use cocoa::base::{id, nil, selector};
use cocoa::foundation::{NSAutoreleasePool, NSString};
use core_foundation::base::kCFAllocatorDefault;
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::runloop::{
    kCFRunLoopCommonModes, CFRunLoopAddTimer, CFRunLoopGetCurrent, CFRunLoopTimerContext,
    CFRunLoopTimerCreate, CFRunLoopTimerRef,
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
    UpdateControlSurface(ControlSurfaceState),
    UpdateHistoryState(bool),
    InjectDiff {
        backspaces: usize,
        suffix: String,
    },
    OpenHistoryFolder(PathBuf),
    Shutdown,
    TogglePanel,
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

    fn update_models(&self, view: &PlatformView) {
        let _ = self
            .cmd_tx
            .send(MacUiCommand::UpdateControlSurface(view.clone()));
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

extern "C" fn on_toggle_panel(_this: &Object, _cmd: Sel, sender: id) {
    unsafe {
        if !MAC_UI_PTR.is_null() {
            (*MAC_UI_PTR).toggle_desktop_panel(sender);
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

extern "C" fn on_status_item_clicked(_this: &Object, _cmd: Sel, sender: id) {
    unsafe {
        if !MAC_UI_PTR.is_null() {
            (*MAC_UI_PTR).handle_status_item_click(sender);
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
                sel!(onTogglePanel:),
                on_toggle_panel as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(onOpenHistoryFolder:),
                on_open_history_folder as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(sel!(onQuit:), on_quit as extern "C" fn(&Object, Sel, id));
            decl.add_method(
                sel!(onStatusItemClicked:),
                on_status_item_clicked as extern "C" fn(&Object, Sel, id),
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
    running: Arc<AtomicBool>,
    controller: DesktopPanelController,
}

static mut MAC_UI_PTR: *mut MacUi = std::ptr::null_mut();

extern "C" fn timer_callback(_timer: CFRunLoopTimerRef, _info: *mut c_void) {
    unsafe {
        if !MAC_UI_PTR.is_null() {
            (*MAC_UI_PTR).drain_commands();
            (*MAC_UI_PTR).check_focus_loss();
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

            let button = status_item.button();
            if button != nil {
                let _: () = msg_send![button, setTarget:delegate];
                let _: () = msg_send![button, setAction:sel!(onStatusItemClicked:)];
                let _: () = msg_send![button, sendActionOn:(1 << 1) | (1 << 3)]; // LeftMouseUp | RightMouseUp
            }

            let controller = DesktopPanelController::new(action_tx.clone());

            let ui = Self {
                action_tx,
                cmd_rx,
                status_item,
                delegate,
                listening: false,
                history_enabled: false,
                running: Arc::new(AtomicBool::new(true)),
                controller,
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

    pub fn check_focus_loss(&mut self) {
        if self.controller.is_visible() {
            unsafe {
                let app = NSApp();
                let is_active: bool = msg_send![app, isActive];
                if !is_active {
                    self.controller.hide_panel();
                }
            }
        }
    }

    pub fn handle_command(&mut self, cmd: MacUiCommand) {
        match cmd {
            MacUiCommand::SetListening(listening) => {
                self.listening = listening;
                self.update_status_bar();
            }
            MacUiCommand::UpdateControlSurface(state) => {
                self.history_enabled = state.history_enabled;
                self.listening = state.runtime_state.is_listening();
                self.update_status_bar();
                self.controller.update_state(&state);
            }
            MacUiCommand::UpdateHistoryState(enabled) => {
                self.history_enabled = enabled;
            }
            MacUiCommand::InjectDiff { backspaces, suffix } => {
                execute_diff(backspaces, &suffix);
            }
            MacUiCommand::OpenHistoryFolder(path) => {
                let _ = Command::new("open").arg(&path).spawn();
            }
            MacUiCommand::Shutdown => {
                self.running.store(false, Ordering::SeqCst);
                self.controller.hide_panel();
                unsafe {
                    let app = NSApp();
                    let _: () = msg_send![app, terminate:nil];
                }
            }
            MacUiCommand::TogglePanel => {
                let button = unsafe { self.status_item.button() };
                self.toggle_desktop_panel(button);
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

    pub fn handle_status_item_click(&mut self, button: id) {
        unsafe {
            let event: id = msg_send![NSApp(), currentEvent];
            let event_type: usize = if event != nil {
                msg_send![event, type]
            } else {
                0
            };

            // Event type 3 is NSRightMouseUp
            if event_type == 3 {
                self.show_fallback_menu(button);
            } else {
                self.toggle_desktop_panel(button);
            }
        }
    }

    pub fn toggle_desktop_panel(&mut self, button: id) {
        if self.controller.is_visible() {
            self.controller.hide_panel();
            return;
        }

        unsafe {
            let pool = NSAutoreleasePool::new(nil);

            let (anchor, screen_bounds) = if button != nil {
                let window: id = msg_send![button, window];
                let frame: cocoa::foundation::NSRect = if window != nil {
                    msg_send![window, frame]
                } else {
                    cocoa::foundation::NSRect {
                        origin: cocoa::foundation::NSPoint { x: 100.0, y: 800.0 },
                        size: cocoa::foundation::NSSize { width: 24.0, height: 24.0 },
                    }
                };

                let screen: id = if window != nil {
                    msg_send![window, screen]
                } else {
                    nil
                };

                let screen_frame: cocoa::foundation::NSRect = if screen != nil {
                    msg_send![screen, frame]
                } else {
                    cocoa::foundation::NSRect {
                        origin: cocoa::foundation::NSPoint { x: 0.0, y: 0.0 },
                        size: cocoa::foundation::NSSize { width: 1440.0, height: 900.0 },
                    }
                };

                let anchor_rect = Rect::new(
                    frame.origin.x as i32,
                    (screen_frame.size.height - (frame.origin.y + frame.size.height)) as i32,
                    frame.size.width as u32,
                    frame.size.height as u32,
                );

                let screen_rect = Rect::new(
                    screen_frame.origin.x as i32,
                    0,
                    screen_frame.size.width as u32,
                    screen_frame.size.height as u32,
                );

                (anchor_rect, screen_rect)
            } else {
                (
                    Rect::new(100, 24, 24, 24),
                    Rect::new(0, 0, 1440, 900),
                )
            };

            let pos = calculate_macos_panel_position(
                anchor,
                screen_bounds,
                PANEL_WIDTH_PX,
                PANEL_HEIGHT_PX,
            );

            self.controller.set_position(pos);
            let _ = self.controller.show_panel();

            let _ = NSAutoreleasePool::drain(pool);
        }
    }

    fn show_fallback_menu(&self, _button: id) {
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let menu = NSMenu::new(nil).autorelease();
            let _: () = msg_send![menu, setAutoenablesItems:false];

            let key_equiv = NSString::alloc(nil).init_str("");

            // 1. Toggle Panel
            let toggle_panel_str = NSString::alloc(nil).init_str("Open Echolet");
            let item: id = msg_send![menu, addItemWithTitle:toggle_panel_str action:sel!(onTogglePanel:) keyEquivalent:key_equiv];
            let _: () = msg_send![item, setTarget:self.delegate];
            let _: () = msg_send![item, setEnabled:true];

            // 2. Start / Stop Listening
            let toggle_label = if self.listening {
                "Stop Listening (F10)"
            } else {
                "Start Listening (F10)"
            };
            let toggle_str = NSString::alloc(nil).init_str(toggle_label);
            let item: id = msg_send![menu, addItemWithTitle:toggle_str action:sel!(onToggleListening:) keyEquivalent:key_equiv];
            let _: () = msg_send![item, setTarget:self.delegate];
            let _: () = msg_send![item, setEnabled:true];

            let _: () = msg_send![menu, addItem:NSMenuItem::separatorItem(nil)];

            // 3. Open History Folder
            let open_str = NSString::alloc(nil).init_str("Open History Folder");
            let item: id = msg_send![menu, addItemWithTitle:open_str action:sel!(onOpenHistoryFolder:) keyEquivalent:key_equiv];
            let _: () = msg_send![item, setTarget:self.delegate];
            let _: () = msg_send![item, setEnabled:true];

            let _: () = msg_send![menu, addItem:NSMenuItem::separatorItem(nil)];

            // 4. Quit
            let quit_str = NSString::alloc(nil).init_str("Quit");
            let item: id = msg_send![menu, addItemWithTitle:quit_str action:sel!(onQuit:) keyEquivalent:key_equiv];
            let _: () = msg_send![item, setTarget:self.delegate];
            let _: () = msg_send![item, setEnabled:true];

            let _: () = msg_send![self.status_item, popUpStatusItemMenu:menu];

            let _ = NSAutoreleasePool::drain(pool);
        }
    }

    pub fn run(mut self) {
        unsafe {
            MAC_UI_PTR = &mut self as *mut MacUi;

            self.update_status_bar();

            // Register Carbon global F10
            let _hotkey_handle = register_global_f10(self.action_tx.clone());

            // Install CFRunLoopTimer for 15ms main thread command draining and focus loss check
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
