use crate::actions::AppAction;
use crate::paths;
use crate::platform::windows::hotkey::{register_f10, unregister_f10, HOTKEY_F10_ID};
use crate::platform::windows::icon;
use crate::platform::{PlatformHandle, PlatformView};
use crate::ui::control_surface::ControlSurfaceState;
use crate::ui::desktop::adapter::{PANEL_HEIGHT_PX, PANEL_WIDTH_PX};
use crate::ui::desktop::controller::{DesktopPanelHandle, DesktopPanelRuntime};
use crate::ui::desktop::host::{calculate_windows_panel_position, Point, Rect};
use crossbeam_channel::{Receiver, Sender};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallWindowProcW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetWindowLongPtrW, TrackPopupMenuEx, TranslateMessage, GWLP_WNDPROC, HICON, HMENU,
    HWND_MESSAGE, MF_SEPARATOR, MF_STRING, MSG, SW_SHOWNORMAL, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY,
    WM_KILLFOCUS, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WNDPROC,
};

pub enum WindowsUiCommand {
    SetListening(bool),
    UpdateControlSurface(Box<ControlSurfaceState>),
    UpdateHistoryState(bool),
    OpenHistoryFolder(PathBuf),
    TogglePanel,
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

    fn update_models(&self, view: &PlatformView) {
        let _ = self
            .cmd_tx
            .send(WindowsUiCommand::UpdateControlSurface(Box::new(
                view.clone(),
            )));
        self.notify_ui();
    }

    fn update_history_state(&self, enabled: bool) {
        let _ = self
            .cmd_tx
            .send(WindowsUiCommand::UpdateHistoryState(enabled));
        self.notify_ui();
    }

    fn open_history_folder(&self, history_dir: &Path) {
        let _ = self.cmd_tx.send(WindowsUiCommand::OpenHistoryFolder(
            history_dir.to_path_buf(),
        ));
        self.notify_ui();
    }
}

const WM_APP_WAKEUP: u32 = WM_APP + 1;
const WM_TRAY_CALLBACK: u32 = WM_APP + 2;
const TRAY_ICON_ID: u32 = 1001;

const IDM_TOGGLE_PANEL: usize = 2000;
const IDM_TOGGLE_LISTENING: usize = 2001;
const IDM_OPEN_HISTORY_FOLDER: usize = 2004;
const IDM_QUIT: usize = 2007;

struct UiState {
    action_tx: Sender<AppAction>,
    cmd_rx: Receiver<WindowsUiCommand>,
    listening: bool,
    history_enabled: bool,
    taskbar_created_msg: u32,
    icon_standby: HICON,
    icon_listening: HICON,
    panel_handle: DesktopPanelHandle,
}

static mut UI_STATE_PTR: *mut UiState = ptr::null_mut();
static mut PREV_PANEL_WNDPROC: Option<WNDPROC> = None;
static mut SUBCLASSED_PANEL_HWND: HWND = ptr::null_mut();

unsafe extern "system" fn panel_subclass_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if (msg == WM_ACTIVATE && (wparam as u32 & 0xFFFF) == WA_INACTIVE) || msg == WM_KILLFOCUS {
        if !UI_STATE_PTR.is_null() {
            (*UI_STATE_PTR).panel_handle.focus_lost();
        }
    }
    if let Some(prev) = PREV_PANEL_WNDPROC {
        CallWindowProcW(prev, hwnd, msg, wparam, lparam)
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

unsafe fn ensure_panel_subclassed() {
    if SUBCLASSED_PANEL_HWND.is_null() {
        let hwnd = FindWindowW(ptr::null(), to_wide("Echolet").as_ptr());
        if !hwnd.is_null() {
            SUBCLASSED_PANEL_HWND = hwnd;
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, panel_subclass_wndproc as isize);
            if prev != 0 {
                PREV_PANEL_WNDPROC = Some(std::mem::transmute(prev));
            }
        }
    }
}

unsafe fn get_windows_tray_position(hwnd: HWND) -> Point {
    let mut identifier: NOTIFYICONIDENTIFIER = std::mem::zeroed();
    identifier.cbSize = size_of::<NOTIFYICONIDENTIFIER>() as u32;
    identifier.hWnd = hwnd;
    identifier.uID = TRAY_ICON_ID;

    let mut icon_rect: RECT = std::mem::zeroed();
    let hr = Shell_NotifyIconGetRect(&identifier, &mut icon_rect);

    if hr == 0 {
        let tray = Rect::new(
            icon_rect.left,
            icon_rect.top,
            (icon_rect.right - icon_rect.left).max(1) as u32,
            (icon_rect.bottom - icon_rect.top).max(1) as u32,
        );
        let hmonitor = MonitorFromRect(&icon_rect, MONITOR_DEFAULTTONEAREST);
        let work_area = get_monitor_work_area(hmonitor);
        calculate_windows_panel_position(tray, work_area, PANEL_WIDTH_PX, PANEL_HEIGHT_PX)
    } else {
        // Fallback: cursor position when Shell_NotifyIconGetRect is unsupported or fails
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        let cursor_rect = Rect::new(pt.x, pt.y, 24, 24);
        let hmonitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let work_area = get_monitor_work_area(hmonitor);
        calculate_windows_panel_position(cursor_rect, work_area, PANEL_WIDTH_PX, PANEL_HEIGHT_PX)
    }
}

unsafe fn get_monitor_work_area(hmonitor: HMONITOR) -> Rect {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(hmonitor, &mut mi) != 0 {
        Rect::new(
            mi.rcWork.left,
            mi.rcWork.top,
            (mi.rcWork.right - mi.rcWork.left).max(800) as u32,
            (mi.rcWork.bottom - mi.rcWork.top).max(600) as u32,
        )
    } else {
        Rect::new(0, 0, 1280, 720)
    }
}

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
                    WindowsUiCommand::UpdateControlSurface(surface_state) => {
                        state.listening = surface_state.runtime_state.is_listening();
                        state.history_enabled = surface_state.history_enabled;
                        let icon = if state.listening {
                            state.icon_listening
                        } else {
                            state.icon_standby
                        };
                        update_tray_icon(hwnd, state.listening, NIM_MODIFY, icon);
                        state.panel_handle.update_state(&surface_state);
                    }
                    WindowsUiCommand::UpdateHistoryState(enabled) => {
                        state.history_enabled = enabled;
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
                    WindowsUiCommand::TogglePanel => {
                        ensure_panel_subclassed();
                        let pos = get_windows_tray_position(hwnd);
                        state.panel_handle.set_position(pos);
                        state.panel_handle.toggle_panel(Some(pos));
                    }
                    WindowsUiCommand::Shutdown => {
                        state.panel_handle.shutdown();
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
            if event == WM_LBUTTONUP {
                ensure_panel_subclassed();
                let pos = get_windows_tray_position(hwnd);
                state.panel_handle.set_position(pos);
                state.panel_handle.toggle_panel(Some(pos));
            } else if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                show_tray_menu(hwnd, state);
            }
            0
        }
        WM_DESTROY => {
            state.panel_handle.shutdown();
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

unsafe fn show_tray_menu(hwnd: HWND, state: &mut UiState) {
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);

    let hmenu: HMENU = CreatePopupMenu();
    if hmenu.is_null() {
        return;
    }

    // 1. Open Echolet
    AppendMenuW(
        hmenu,
        MF_STRING,
        IDM_TOGGLE_PANEL,
        to_wide("Open Echolet").as_ptr(),
    );

    // 2. Start / Stop Listening
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

    // 3. Open History Folder
    AppendMenuW(
        hmenu,
        MF_STRING,
        IDM_OPEN_HISTORY_FOLDER,
        to_wide("Open History Folder").as_ptr(),
    );
    AppendMenuW(hmenu, MF_SEPARATOR, 0, ptr::null());

    // 4. Quit
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
        IDM_TOGGLE_PANEL => {
            ensure_panel_subclassed();
            let pos = get_windows_tray_position(hwnd);
            state.panel_handle.set_position(pos);
            state.panel_handle.toggle_panel(Some(pos));
        }
        IDM_TOGGLE_LISTENING => {
            let _ = state.action_tx.send(AppAction::ToggleListening);
        }
        IDM_OPEN_HISTORY_FOLDER => {
            let _ = state.action_tx.send(AppAction::OpenHistoryFolder);
        }
        IDM_QUIT => {
            state.panel_handle.shutdown();
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

            let wnd_class = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };

            RegisterClassW(&wnd_class);

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

            let (panel_handle, panel_init) = DesktopPanelRuntime::init(action_tx.clone());

            // Spawn dedicated Slint UI event loop thread (single-thread owner)
            let _ = thread::Builder::new()
                .name("echolet-slint-ui".into())
                .spawn(move || {
                    let runtime = DesktopPanelRuntime::from_init(panel_init);
                    if let Err(e) = runtime.run() {
                        eprintln!("[Windows UI] Slint event loop error: {}", e);
                    }
                });

            let mut state = UiState {
                action_tx,
                cmd_rx,
                listening: false,
                history_enabled: false,
                taskbar_created_msg: taskbar_msg,
                icon_standby,
                icon_listening,
                panel_handle,
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
