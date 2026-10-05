//! Shared Desktop Panel Controller (PROJECT-041 / J11.1d).
//!
//! Manages the lifecycle, callback wiring, and visibility of the shared [`EcholetPanel`].

use crate::actions::AppAction;
use crate::ui::control_surface::{dispatch_surface_action, ControlSurfaceState, SurfaceAction};
use crate::ui::desktop::adapter::{DesktopPanelViewModel, EcholetPanel, SlintControlSurfaceAdapter};
use crate::ui::desktop::host::Point;
use crossbeam_channel::Sender;
use slint::ComponentHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct DesktopPanelController {
    panel: Option<EcholetPanel>,
    action_tx: Sender<AppAction>,
    current_state: Arc<Mutex<ControlSurfaceState>>,
    is_open: Arc<AtomicBool>,
}

impl DesktopPanelController {
    pub fn new(action_tx: Sender<AppAction>) -> Self {
        Self {
            panel: None,
            action_tx,
            current_state: Arc::new(Mutex::new(ControlSurfaceState::default())),
            is_open: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Initializes and caches the shared Slint panel lazily on first access.
    pub fn ensure_panel(&mut self) -> Result<&EcholetPanel, Box<dyn std::error::Error>> {
        if self.panel.is_none() {
            let panel = EcholetPanel::new()?;

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
                let state_guard = state_for_model.lock().unwrap();
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
            let state_guard = self.current_state.lock().unwrap();
            let vm = DesktopPanelViewModel::from_control_surface(&state_guard);
            SlintControlSurfaceAdapter::apply_to_panel(&panel, &vm);

            self.panel = Some(panel);
        }

        Ok(self.panel.as_ref().unwrap())
    }

    /// Shows the panel and sets open state.
    pub fn show_panel(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_panel()?;
        if let Some(ref panel) = self.panel {
            panel.show()?;
            self.is_open.store(true, Ordering::SeqCst);
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
    pub fn toggle_panel(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_visible() {
            self.hide_panel();
            Ok(())
        } else {
            self.show_panel()
        }
    }

    /// Whether the panel is currently open/visible.
    pub fn is_visible(&self) -> bool {
        self.is_open.load(Ordering::SeqCst)
    }

    /// Updates the panel's data from canonical [`ControlSurfaceState`].
    pub fn update_state(&mut self, state: &ControlSurfaceState) {
        {
            let mut state_guard = self.current_state.lock().unwrap();
            *state_guard = state.clone();
        }
        if let Some(ref panel) = self.panel {
            let vm = DesktopPanelViewModel::from_control_surface(state);
            SlintControlSurfaceAdapter::apply_to_panel(panel, &vm);
        }
    }

    /// Sets the window position.
    pub fn set_position(&self, pos: Point) {
        if let Some(ref panel) = self.panel {
            panel
                .window()
                .set_position(slint::PhysicalPosition::new(pos.x, pos.y));
        }
    }
}

// Safety: DesktopPanelController encapsulates EcholetPanel behind thread-safe synchronization.
// All mutable access and window interactions are serialized across threads via Mutex.
unsafe impl Send for DesktopPanelController {}

