use crate::actions::AppAction;
use crate::paths;
use crate::platform::windows::hotkey::{register_f10, unregister_f10, HOTKEY_F10_ID};
use crate::platform::windows::icon;
use crate::platform::PlatformHandle;
use crossbeam_channel::{Receiver, Sender};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, HICON, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, TrackPopupMenuEx, TranslateMessage, HWND_MESSAGE, HMENU,
    MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY,
    WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, SW_SHOWNORMAL,
};

pub enum WindowsUiCommand {
    SetListening(bool),
    UpdateModels {
        active_id: String,
        installed_ids: Vec<String>,
        downloading_ids: Vec<String>,
    },
    UpdateHistoryState(bool),
    OpenHistoryFolder(PathBuf),
    Shutdown,
}

pub struct WindowsPlatformHandle {
    cmd_tx: Sender<WindowsUiCommand>,
    hwnd: Arc<AtomicIsize>,
}

impl WindowsPlatformHandle {
    pub fn new(cmd_tx: Sender<WindowsUiCommand>, hwnd: Arc<AtomicIsize>) -> Self {
        Self { cmd_tx, hwnd }
    }

    fn notify_ui(&self) {
        let h = self.hwnd.load(Ordering::SeqCst);
        if h != 0 {
            unsafe {
                PostMessageW(h as HWND, WM_APP_WAKEUP, 0, 0);
            }
        }
    }
}

impl PlatformHandle for WindowsPlatformHandle {
    fn set_listening(&self, listening: bool) {
        let _ = self.cmd_tx.send(WindowsUiCommand::SetListening(listening));
        self.notify_ui();
    }

    fn shutdown(&self) {
        let _ = self.cmd_tx.send(WindowsUiCommand::Shutdown);
        self.notify_ui();
    }

    fn update_models(
        &self,
        active_id: &str,
        installed_ids: &[String],
        downloading_ids: &[String],
    ) {
        let _ = self.cmd_tx.send(WindowsUiCommand::UpdateModels {
            active_id: active_id.to_string(),
            installed_ids: installed_ids.to_vec(),
            downloading_ids: downloading_ids.to_vec(),
        });
        self.notify_ui();
    }

    fn update_history_state(&self, enabled: bool) {
        let _ = self.cmd_tx.send(WindowsUiCommand::UpdateHistoryState(enabled));
        self.notify_ui();
    }

    fn open_history_folder(&self, history_dir: &Path) {
        let _ = self
            .cmd_tx
            .send(WindowsUiCommand::OpenHistoryFolder(history_dir.to_path_buf()));
        self.notify_ui();
    }
}

const WM_APP_WAKEUP: u32 = WM_APP + 1;
const WM_TRAY_CALLBACK: u32 = WM_APP + 2;
const TRAY_ICON_ID: u32 = 1001;

const IDM_TOGGLE_LISTENING: usize = 2001;
const IDM_MODEL_INFO: usize = 2002;
const IDM_TOGGLE_HISTORY: usize = 2003;
const IDM_OPEN_HISTORY_FOLDER: usize = 2004;
const IDM_HISTORY_PATH: usize = 2005;
const IDM_HOTKEY_INFO: usize = 2006;
const IDM_QUIT: usize = 2007;

struct UiState {
    action_tx: Sender<AppAction>,
    cmd_rx: Receiver<WindowsUiCommand>,
    listening: bool,
    history_enabled: bool,
    model_name: String,
    taskbar_created_msg: u32,
    icon_standby: HICON,
    icon_listening: HICON,
}

static mut UI_STATE_PTR: *mut UiState = ptr::null_mut();

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if UI_STATE_PTR.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *UI_STATE_PTR;

    if msg == state.taskbar_created_msg {
        let icon = if state.listening {
            state.icon_listening
        } else {
            state.icon_standby
        };
        update_tray_icon(hwnd, state.listening, NIM_ADD, icon);
        return 0;
    }

    match msg {
        WM_APP_WAKEUP => {
            while let Ok(cmd) = state.cmd_rx.try_recv() {
                match cmd {
                    WindowsUiCommand::SetListening(listening) => {
                        state.listening = listening;
                        let icon = if listening {
                            state.icon_listening
                        } else {
                            state.icon_standby
                        };
                        update_tray_icon(hwnd, listening, NIM_MODIFY, icon);
                    }
                    WindowsUiCommand::UpdateHistoryState(enabled) => {
                        state.history_enabled = enabled;
                    }
                    WindowsUiCommand::UpdateModels { active_id, .. } => {
                        state.model_name = active_id;
                    }
                    WindowsUiCommand::OpenHistoryFolder(path) => {
                        let mut path_utf16: Vec<u16> =
                            path.to_string_lossy().encode_utf16().collect();
                        path_utf16.push(0);
                        let open_verb: Vec<u16> = "open\0".encode_utf16().collect();
                        ShellExecuteW(
                            ptr::null_mut(),
                            open_verb.as_ptr(),
                            path_utf16.as_ptr(),
                            ptr::null(),
                            ptr::null(),
                            SW_SHOWNORMAL,
                        );
                    }
                    WindowsUiCommand::Shutdown => {
                        DestroyWindow(hwnd);
                    }
                }
            }
            0
        }
        WM_HOTKEY => {
            if wparam == HOTKEY_F10_ID as usize {
                let _ = state.action_tx.send(AppAction::ToggleListening);
            }
            0
        }
        WM_TRAY_CALLBACK => {
            let event = lparam as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_CONTEXTMENU {
                show_tray_menu(hwnd, state);
            }
            0
        }
        WM_DESTROY => {
            update_tray_icon(hwnd, false, NIM_DELETE, state.icon_standby);
            unregister_f10(hwnd);
            if !state.icon_standby.is_null() {
                DestroyIcon(state.icon_standby);
            }
            if !state.icon_listening.is_null() {
                DestroyIcon(state.icon_listening);
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn update_tray_icon(hwnd: HWND, listening: bool, action: u32, icon: HICON) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ICON_ID;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_CALLBACK;
    nid.hIcon = icon;

    let tip = if listening {
        "Echolet (Listening - Press F10 to stop)"
    } else {
        "Echolet (Standby - Press F10 to speak)"
    };
    let tip_wide = to_wide(tip);
    let len = tip_wide.len().min(nid.szTip.len());
    nid.szTip[..len].copy_from_slice(&tip_wide[..len]);

    Shell_NotifyIconW(action, &nid);
}

unsafe fn show_tray_menu(hwnd: HWND, state: &UiState) {
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);

    let hmenu: HMENU = CreatePopupMenu();
    if hmenu.is_null() {
        return;
    }

    // 1. Start/Stop Listening
    let toggle_text = if state.listening {
        "Stop Listening (F10)"
    } else {
        "Start Listening (F10)"
    };
    AppendMenuW(
        hmenu,
        MF_STRING,
        IDM_TOGGLE_LISTENING,
        to_wide(toggle_text).as_ptr(),
    );
    AppendMenuW(hmenu, MF_SEPARATOR, 0, ptr::null());

    // 2. Model Info
    let model_text = format!("Model: {}", state.model_name);
    AppendMenuW(
        hmenu,
        MF_STRING | MF_DISABLED | MF_GRAYED,
        IDM_MODEL_INFO,
        to_wide(&model_text).as_ptr(),
    );
    AppendMenuW(hmenu, MF_SEPARATOR, 0, ptr::null());

    // 3. Local History
    let hist_dir = paths::history_dir()
        .to_string_lossy()
        .replace(&std::env::var("USERPROFILE").unwrap_or_default(), "~");

    if !state.history_enabled {
        AppendMenuW(
            hmenu,
            MF_STRING,
            IDM_TOGGLE_HISTORY,
            to_wide("Local History: Off").as_ptr(),
        );
    } else {
        AppendMenuW(
            hmenu,
            MF_STRING,
            IDM_TOGGLE_HISTORY,
            to_wide("✓ Local History").as_ptr(),
        );
        AppendMenuW(
            hmenu,
            MF_STRING,
            IDM_OPEN_HISTORY_FOLDER,
            to_wide("    Open History Folder").as_ptr(),
        );
        let path_text = format!("    {}", hist_dir);
        AppendMenuW(
            hmenu,
            MF_STRING | MF_DISABLED | MF_GRAYED,
            IDM_HISTORY_PATH,
            to_wide(&path_text).as_ptr(),
        );
    }
    AppendMenuW(hmenu, MF_SEPARATOR, 0, ptr::null());

    // 4. Hotkey
    AppendMenuW(
        hmenu,
        MF_STRING | MF_DISABLED | MF_GRAYED,
        IDM_HOTKEY_INFO,
        to_wide("Hotkey: F10").as_ptr(),
    );
    AppendMenuW(hmenu, MF_SEPARATOR, 0, ptr::null());

    // 5. Quit
    AppendMenuW(hmenu, MF_STRING, IDM_QUIT, to_wide("Quit").as_ptr());

    let selected = TrackPopupMenuEx(
        hmenu,
        TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_RETURNCMD,
        pt.x,
        pt.y,
        hwnd,
        ptr::null(),
    ) as usize;

    DestroyMenu(hmenu);

    match selected {
        IDM_TOGGLE_LISTENING => {
            let _ = state.action_tx.send(AppAction::ToggleListening);
        }
        IDM_TOGGLE_HISTORY => {
            let _ = state.action_tx.send(AppAction::ToggleHistory);
        }
        IDM_OPEN_HISTORY_FOLDER => {
            let _ = state.action_tx.send(AppAction::OpenHistoryFolder);
        }
        IDM_QUIT => {
            let _ = state.action_tx.send(AppAction::Quit);
        }
        _ => {}
    }
}

pub fn spawn_ui_thread(
    action_tx: Sender<AppAction>,
    cmd_rx: Receiver<WindowsUiCommand>,
    hwnd_out: Arc<AtomicIsize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (init_tx, init_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);

    thread::Builder::new()
        .name("echolet-win32-ui".into())
        .spawn(move || unsafe {
            let class_name = to_wide("EcholetMessageWindowClass");
            let hinstance = GetModuleHandleW(ptr::null());

            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wnd_proc);
            wc.hInstance = hinstance;
            wc.lpszClassName = class_name.as_ptr();

            if RegisterClassW(&wc) == 0 {
                let _ = init_tx.send(Err("Failed to register window class".into()));
                return;
            }

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                hinstance,
                ptr::null(),
            );

            if hwnd.is_null() {
                let _ = init_tx.send(Err("Failed to create message-only window".into()));
                return;
            }

            hwnd_out.store(hwnd as isize, Ordering::SeqCst);

            let taskbar_msg = RegisterWindowMessageW(to_wide("TaskbarCreated").as_ptr());

            let icon_standby = icon::create_echolet_icon(false);
            let icon_listening = icon::create_echolet_icon(true);
            if icon_standby.is_null() || icon_listening.is_null() {
                eprintln!("[Platform] Failed to create custom tray icon(s).");
            }

            let mut state = UiState {
                action_tx,
                cmd_rx,
                listening: false,
                history_enabled: false,
                model_name: "Chinese + English (X-ASR / 480ms)".into(),
                taskbar_created_msg: taskbar_msg,
                icon_standby,
                icon_listening,
            };

            UI_STATE_PTR = &mut state;

            // 1. Add tray icon (standby state)
            update_tray_icon(hwnd, false, NIM_ADD, state.icon_standby);

            // 2. Register F10 hotkey
            register_f10(hwnd);

            let _ = init_tx.send(Ok(()));

            // 3. Win32 Message Loop
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            UI_STATE_PTR = ptr::null_mut();
        })?;

    init_rx.recv()??;
    Ok(())
}
