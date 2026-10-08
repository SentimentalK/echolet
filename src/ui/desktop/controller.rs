//! Shared Desktop Panel Architecture (PROJECT-041 / J11.1d-R1).
//!
//! Enforces strict single-thread UI ownership of [`EcholetPanel`] through
//! [`DesktopPanelRuntime`], while exposing a lightweight, command-based [`DesktopPanelHandle`]
//! that is safely `Send + Sync` for cross-thread coordination.

use crate::actions::AppAction;
use crate::ui::control_surface::{dispatch_surface_action, ControlSurfaceState, SurfaceAction};
use crate::ui::desktop::adapter::{
    DesktopPanelViewModel, EcholetPanel, SlintControlSurfaceAdapter,
};
use crate::ui::desktop::host::{DesktopPanelHost, Point};
use crossbeam_channel::{Receiver, Sender};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

thread_local! {
    static ACTIVE_RUNTIME: RefCell<Option<*mut DesktopPanelRuntime>> = const { RefCell::new(None) };
}

/// Helper invoked on the Slint event loop thread to immediately drain pending commands.
pub fn drain_active_runtime() {
    ACTIVE_RUNTIME.with(|cell| {
        if let Some(ptr) = *cell.borrow() {
            unsafe {
                (*ptr).drain_commands();
            }
        }
    });
}

/// Asynchronous commands dispatched from worker/platform threads to the UI owner thread.
#[derive(Debug, Clone)]
pub enum DesktopPanelCommand {
    Show(Option<Point>),
    Hide,
    Toggle(Option<Point>),
    SetPosition(Point),
    UpdateState(Box<ControlSurfaceState>),
    FocusLost,
    Shutdown,
}

/// Sendable, thread-safe command-side handle.
///
/// Contains NO strong reference to `EcholetPanel` or any live UI component.
/// All mutations are enqueued and marshalled to the UI owner thread.
#[derive(Clone)]
pub struct DesktopPanelHandle {
    cmd_tx: Sender<DesktopPanelCommand>,
    is_open: Arc<AtomicBool>,
}

impl DesktopPanelHandle {
    pub fn new(cmd_tx: Sender<DesktopPanelCommand>, is_open: Arc<AtomicBool>) -> Self {
        Self { cmd_tx, is_open }
    }

    /// Whether the panel is currently open/visible (atomic query).
    pub fn is_visible(&self) -> bool {
        self.is_open.load(Ordering::SeqCst)
    }

    /// Enqueues a command to show the panel at the specified position.
    pub fn show_panel(&self, pos: Option<Point>) {
        let _ = self.cmd_tx.send(DesktopPanelCommand::Show(pos));
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }

    /// Enqueues a command to hide the panel.
    pub fn hide_panel(&self) {
        let _ = self.cmd_tx.send(DesktopPanelCommand::Hide);
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }

    /// Enqueues a command to toggle the panel visibility.
    pub fn toggle_panel(&self, pos: Option<Point>) {
        let _ = self.cmd_tx.send(DesktopPanelCommand::Toggle(pos));
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }

    /// Enqueues a command to update the panel position.
    pub fn set_position(&self, pos: Point) {
        let _ = self.cmd_tx.send(DesktopPanelCommand::SetPosition(pos));
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }

    /// Enqueues a canonical [`ControlSurfaceState`] update.
    pub fn update_state(&self, state: &ControlSurfaceState) {
        let _ = self
            .cmd_tx
            .send(DesktopPanelCommand::UpdateState(Box::new(state.clone())));
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }

    /// Enqueues a focus-loss / outside-click dismissal notification.
    pub fn focus_lost(&self) {
        let _ = self.cmd_tx.send(DesktopPanelCommand::FocusLost);
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }

    /// Enqueues a shutdown command.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(DesktopPanelCommand::Shutdown);
        let _ = slint::invoke_from_event_loop(drain_active_runtime);
    }
}

impl DesktopPanelHost for DesktopPanelHandle {
    fn toggle_panel(&self) {
        self.toggle_panel(None);
    }

    fn show_panel(&self) {
        self.show_panel(None);
    }

    fn hide_panel(&self) {
        self.hide_panel();
    }

    fn update_state(&self, state: &ControlSurfaceState) {
        self.update_state(state);
    }
}

/// Sendable initialization data used across thread boundaries to instantiate [`DesktopPanelRuntime`]
/// on its designated owner thread.
///
/// Contains ONLY thread-safe primitives (`Send`), with no UI components, references, or `Rc` handles.
pub struct DesktopPanelInit {
    pub cmd_rx: Receiver<DesktopPanelCommand>,
    pub action_tx: Sender<AppAction>,
    pub is_open: Arc<AtomicBool>,
    pub initial_state: Option<ControlSurfaceState>,
}

impl DesktopPanelInit {
    /// Creates a linked (Handle, Init) pair.
    pub fn new(action_tx: Sender<AppAction>) -> (DesktopPanelHandle, Self) {
        DesktopPanelRuntime::init(action_tx)
    }

    /// Builds the runtime on the current owner thread.
    pub fn build(self) -> DesktopPanelRuntime {
        DesktopPanelRuntime::from_init(self)
    }
}

/// UI-thread-bound runtime owner of the live [`EcholetPanel`].
///
/// Intentionally `!Send` and `!Sync` via `PhantomData<*mut ()>`.
/// Must only be instantiated, stored, and operated on the valid Slint event loop thread.
///
/// ```compile_fail
/// use echolet::ui::desktop::controller::DesktopPanelRuntime;
/// fn assert_send<T: Send>() {}
/// assert_send::<DesktopPanelRuntime>();
/// ```
pub struct DesktopPanelRuntime {
    panel: Option<EcholetPanel>,
    action_tx: Sender<AppAction>,
    cmd_rx: Receiver<DesktopPanelCommand>,
    current_state: Rc<RefCell<ControlSurfaceState>>,
    is_open: Arc<AtomicBool>,
    _thread_bound: PhantomData<*mut ()>,
}

impl DesktopPanelRuntime {
    /// Direct constructor for threads that are themselves the UI owner thread (e.g. macOS main thread).
    pub fn new(action_tx: Sender<AppAction>) -> (DesktopPanelHandle, Self) {
        let (handle, init) = Self::init(action_tx);
        let runtime = Self::from_init(init);
        (handle, runtime)
    }

    /// Creates the sendable handle and sendable initialization payload without constructing `DesktopPanelRuntime`.
    ///
    /// The returned [`DesktopPanelInit`] can be safely sent across thread boundaries to the thread
    /// that will host the Slint event loop and instantiate [`DesktopPanelRuntime`].
    pub fn init(action_tx: Sender<AppAction>) -> (DesktopPanelHandle, DesktopPanelInit) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let is_open = Arc::new(AtomicBool::new(false));
        let handle = DesktopPanelHandle::new(cmd_tx, is_open.clone());
        let init = DesktopPanelInit {
            cmd_rx,
            action_tx,
            is_open,
            initial_state: None,
        };
        (handle, init)
    }

    /// Constructs the thread-bound runtime from [`DesktopPanelInit`] on the designated owner thread.
    pub fn from_init(init: DesktopPanelInit) -> Self {
        let initial_state = init.initial_state.unwrap_or_default();
        Self {
            panel: None,
            action_tx: init.action_tx,
            cmd_rx: init.cmd_rx,
            current_state: Rc::new(RefCell::new(initial_state)),
            is_open: init.is_open,
            _thread_bound: PhantomData,
        }
    }

    /// Access the current control surface state snapshot.
    pub fn current_state(&self) -> ControlSurfaceState {
        self.current_state.borrow().clone()
    }

    /// Whether the panel is currently open/visible.
    pub fn is_visible(&self) -> bool {
        self.is_open.load(Ordering::SeqCst)
    }

    /// Initializes and caches the shared Slint panel lazily on first access.
    pub fn ensure_panel(&mut self) -> Result<&EcholetPanel, Box<dyn std::error::Error>> {
        if self.panel.is_none() {
            #[cfg(target_os = "macos")]
            {
                // On macOS, winit EventLoop must be created on the main thread.
                // In headless tests or secondary threads, return Err instead of panicking.
                let is_main = unsafe { libc::pthread_main_np() != 0 };
                if !is_main {
                    return Err(
                        "Slint window cannot be initialized off the main thread on macOS".into(),
                    );
                }
            }

            let panel_res =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(EcholetPanel::new));
            let panel = match panel_res {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => return Err(Box::new(e)),
                Err(_) => {
                    return Err(
                        "Failed to create EcholetPanel (panicked during window creation)".into(),
                    )
                }
            };

            // Wire callbacks
            let is_open_weak = self.is_open.clone();
            let panel_weak_close = panel.as_weak();
            panel.on_close_requested(move || {
                if let Some(p) = panel_weak_close.upgrade() {
                    let _ = p.hide();
                }
                is_open_weak.store(false, Ordering::SeqCst);
            });

            let tx_toggle = self.action_tx.clone();
            panel.on_toggle_listening(move || {
                dispatch_surface_action(&tx_toggle, SurfaceAction::ToggleListening);
            });

            let tx_model = self.action_tx.clone();
            let state_for_model = self.current_state.clone();
            panel.on_model_action_clicked(move |id| {
                let id_str = id.to_string();
                let state_guard = state_for_model.borrow();
                let vm = DesktopPanelViewModel::from_control_surface(&state_guard);
                if let Some(action) = vm.resolve_surface_action(&id_str) {
                    dispatch_surface_action(&tx_model, action);
                }
            });

            let tx_lang = self.action_tx.clone();
            panel.on_language_selected(move |model_id, locale| {
                let m_id = model_id.to_string();
                let loc = if locale.is_empty() {
                    None
                } else {
                    Some(locale.to_string())
                };
                dispatch_surface_action(
                    &tx_lang,
                    SurfaceAction::SelectLanguage {
                        model_id: m_id,
                        locale: loc,
                    },
                );
            });

            let tx_preload = self.action_tx.clone();
            panel.on_set_preload_startup(move |val| {
                dispatch_surface_action(&tx_preload, SurfaceAction::SetPreloadModelOnStartup(val));
            });

            let tx_idle = self.action_tx.clone();
            panel.on_set_idle_unload(move |mins| {
                let val = if mins < 0 { None } else { Some(mins as u32) };
                dispatch_surface_action(&tx_idle, SurfaceAction::SetModelIdleUnloadMinutes(val));
            });

            let tx_hist = self.action_tx.clone();
            panel.on_set_history_enabled(move |val| {
                dispatch_surface_action(&tx_hist, SurfaceAction::SetHistoryEnabled(val));
            });

            let tx_open_hist = self.action_tx.clone();
            panel.on_open_history_folder(move || {
                dispatch_surface_action(&tx_open_hist, SurfaceAction::OpenHistoryFolder);
            });

            // Initial view model projection
            let state_guard = self.current_state.borrow();
            let vm = DesktopPanelViewModel::from_control_surface(&state_guard);
            SlintControlSurfaceAdapter::apply_to_panel(&panel, &vm);

            self.panel = Some(panel);
        }

        Ok(self.panel.as_ref().unwrap())
    }

    /// Drains and applies all queued [`DesktopPanelCommand`]s on this owner thread.
    pub fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.apply_command(cmd);
        }
    }

    /// Applies a single [`DesktopPanelCommand`] synchronously on the owner thread.
    pub fn apply_command(&mut self, cmd: DesktopPanelCommand) {
        match cmd {
            DesktopPanelCommand::Show(pos) => {
                if let Some(p) = pos {
                    self.set_position(p);
                }
                let _ = self.show_panel();
            }
            DesktopPanelCommand::Hide => {
                self.hide_panel();
            }
            DesktopPanelCommand::Toggle(pos) => {
                if self.is_visible() {
                    self.hide_panel();
                } else {
                    if let Some(p) = pos {
                        self.set_position(p);
                    }
                    let _ = self.show_panel();
                }
            }
            DesktopPanelCommand::SetPosition(pos) => {
                self.set_position(pos);
            }
            DesktopPanelCommand::UpdateState(state) => {
                self.update_state(*state);
            }
            DesktopPanelCommand::FocusLost => {
                if self.is_visible() {
                    self.hide_panel();
                }
            }
            DesktopPanelCommand::Shutdown => {
                self.hide_panel();
                let _ = slint::quit_event_loop();
            }
        }
    }

    /// Shows the panel and sets open state.
    pub fn show_panel(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.is_open.store(true, Ordering::SeqCst);
        if let Ok(panel) = self.ensure_panel() {
            panel.show()?;
        }
        Ok(())
    }

    /// Hides the panel.
    pub fn hide_panel(&mut self) {
        if let Some(ref panel) = self.panel {
            let _ = panel.hide();
        }
        self.is_open.store(false, Ordering::SeqCst);
    }

    /// Toggles the panel open or closed.
    pub fn toggle_panel(&mut self, pos: Option<Point>) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_visible() {
            self.hide_panel();
            Ok(())
        } else {
            if let Some(p) = pos {
                self.set_position(p);
            }
            self.show_panel()
        }
    }

    /// Updates the panel's data from canonical [`ControlSurfaceState`].
    pub fn update_state(&mut self, state: ControlSurfaceState) {
        *self.current_state.borrow_mut() = state.clone();
        if let Some(ref panel) = self.panel {
            let vm = DesktopPanelViewModel::from_control_surface(&state);
            SlintControlSurfaceAdapter::apply_to_panel(panel, &vm);
        }
    }

    /// Sets the window position.
    pub fn set_position(&mut self, pos: Point) {
        if let Ok(panel) = self.ensure_panel() {
            panel
                .window()
                .set_position(slint::PhysicalPosition::new(pos.x, pos.y));
        }
    }

    /// Runs the Slint event loop on this owner thread until `DesktopPanelCommand::Shutdown`
    /// or `slint::quit_event_loop()` is called.
    ///
    /// The platform tray (macOS status item, Windows tray, Linux tray) is not a Slint
    /// window. `run_event_loop()` returns when the last Slint window is hidden, which
    /// happens as soon as the panel closes and leaves the tray process wedged.
    pub fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_panel()?;

        let self_ptr = &mut self as *mut DesktopPanelRuntime;
        ACTIVE_RUNTIME.with(|cell| {
            *cell.borrow_mut() = Some(self_ptr);
        });

        // 16ms periodic timer as a safety drain and heartbeat for event loop
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(16),
            move || {
                drain_active_runtime();
            },
        );

        let res = slint::run_event_loop_until_quit();

        ACTIVE_RUNTIME.with(|cell| {
            *cell.borrow_mut() = None;
        });

        res.map_err(|e| e.into())
    }
}

/// Backward compatibility alias.
pub type DesktopPanelController = DesktopPanelHandle;
