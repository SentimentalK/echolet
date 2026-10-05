use crate::actions::AppAction;
use crate::config::EcholetConfig;
use crate::models::registry::ModelRegistry;
use crate::paths;
use crate::platform::PlatformHandle;
pub use crate::ui::control_surface::ControlSurfaceState as PlatformView;
use crate::ui::control_surface::{
    dispatch_surface_action, ControlSurfaceState, RuntimeState, SurfaceAction,
};
use crate::ui::desktop::adapter::{PANEL_HEIGHT_PX, PANEL_WIDTH_PX};
use crate::ui::desktop::controller::DesktopPanelController;
use crate::ui::desktop::host::{calculate_linux_panel_position, Rect};
use crossbeam_channel::Sender;
use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, Tray, TrayMethods};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct LinuxTray {
    pub is_listening: Arc<AtomicBool>,
    pub history_enabled: Arc<AtomicBool>,
    pub view: Arc<Mutex<PlatformView>>,
    pub action_tx: Sender<AppAction>,
    pub controller: Arc<Mutex<DesktopPanelController>>,
}

impl Tray for LinuxTray {
    fn id(&self) -> String {
        "voice-input-assistant".into()
    }

    fn title(&self) -> String {
        "Voice Input".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let is_rec = self.is_listening.load(Ordering::SeqCst);
        vec![create_circle_icon(is_rec, 32)]
    }

    fn activate(&mut self, x: i32, y: i32) {
        if let Ok(mut c) = self.controller.lock() {
            let anchor = if x > 0 || y > 0 {
                Some(Rect::new(x, y, 24, 24))
            } else {
                None
            };
            let pos = calculate_linux_panel_position(
                anchor,
                Rect::new(0, 0, 1920, 1080),
                PANEL_WIDTH_PX,
                PANEL_HEIGHT_PX,
            );
            c.set_position(pos);
            let _ = c.toggle_panel();
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let is_rec = self.is_listening.load(Ordering::SeqCst);
        let mut menu_items: Vec<MenuItem<Self>> = Vec::new();

        // 1. Open Echolet
        {
            let controller = self.controller.clone();
            menu_items.push(
                StandardItem {
                    label: "Open Echolet".into(),
                    activate: Box::new(move |_| {
                        if let Ok(mut c) = controller.lock() {
                            let _ = c.toggle_panel();
                        }
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        // 2. Start / Stop Listening
        {
            let tx = self.action_tx.clone();
            menu_items.push(
                StandardItem {
                    label: if is_rec {
                        "Stop Listening (F10)"
                    } else {
                        "Start Listening (F10)"
                    }
                    .into(),
                    activate: Box::new(move |_| {
                        dispatch_surface_action(&tx, SurfaceAction::ToggleListening);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        menu_items.push(MenuItem::Separator);

        // 3. Open History Folder
        {
            let tx = self.action_tx.clone();
            menu_items.push(
                StandardItem {
                    label: "Open History Folder".into(),
                    activate: Box::new(move |_| {
                        dispatch_surface_action(&tx, SurfaceAction::OpenHistoryFolder);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        menu_items.push(MenuItem::Separator);

        // 4. Quit
        {
            let tx = self.action_tx.clone();
            menu_items.push(
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(move |_| {
                        dispatch_surface_action(&tx, SurfaceAction::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        menu_items
    }
}

pub struct LinuxPlatformHandle {
    pub is_listening: Arc<AtomicBool>,
    pub history_enabled: Arc<AtomicBool>,
    pub view: Arc<Mutex<PlatformView>>,
    pub registry: ModelRegistry,
    pub tray_handle: Option<ksni::Handle<LinuxTray>>,
    pub rt: Option<tokio::runtime::Runtime>,
    pub controller: Arc<Mutex<DesktopPanelController>>,
}

impl LinuxPlatformHandle {
    fn refresh(&self) {
        if let Some(handle) = &self.tray_handle {
            if let Some(rt) = &self.rt {
                rt.block_on(async {
                    handle.update(|_| {}).await;
                });
            }
        }
    }
}

impl PlatformHandle for LinuxPlatformHandle {
    fn set_listening(&self, listening: bool) {
        self.is_listening.store(listening, Ordering::SeqCst);
        self.refresh();
    }

    fn shutdown(&self) {
        if let Ok(mut c) = self.controller.lock() {
            c.hide_panel();
        }
        if let Some(handle) = &self.tray_handle {
            handle.shutdown();
        }
    }

    fn update_models(&self, view: &PlatformView) {
        if let Ok(mut lock) = self.view.lock() {
            *lock = view.clone();
        }
        if let Ok(mut c) = self.controller.lock() {
            c.update_state(view);
        }
        self.refresh();
    }

    fn update_history_state(&self, enabled: bool) {
        self.history_enabled.store(enabled, Ordering::SeqCst);
        self.refresh();
    }

    fn open_history_folder(&self, history_dir: &Path) {
        let _ = std::fs::create_dir_all(history_dir);
        let res = std::process::Command::new("xdg-open")
            .arg(history_dir)
            .spawn();
        if let Err(err) = res {
            eprintln!(
                "[Platform] Failed to open folder {:?}: {}",
                history_dir, err
            );
        }
    }
}

/// Neutral starting projection: nothing is assumed installed or selected.
fn initial_platform_view(registry: &ModelRegistry) -> PlatformView {
    let config = EcholetConfig::default();
    crate::platform::build_view(
        registry,
        None,
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &config,
        RuntimeState::NoModel,
    )
}

pub fn spawn_linux_tray(action_tx: Sender<AppAction>) -> LinuxPlatformHandle {
    let is_listening = Arc::new(AtomicBool::new(false));
    let history_enabled = Arc::new(AtomicBool::new(false));

    // Load initial registry for initial menu population
    let res_root = paths::resource_root();
    let reg_path = res_root.join("models/registry.json");
    let registry = if reg_path.exists() {
        ModelRegistry::from_file(&reg_path).unwrap_or_else(|_| {
            ModelRegistry::from_str(include_str!("../../../models/registry.json")).unwrap()
        })
    } else {
        ModelRegistry::from_str(include_str!("../../../models/registry.json")).unwrap()
    };

    let view = Arc::new(Mutex::new(initial_platform_view(&registry)));
    let controller = Arc::new(Mutex::new(DesktopPanelController::new(action_tx.clone())));

    let tray = LinuxTray {
        is_listening: is_listening.clone(),
        history_enabled: history_enabled.clone(),
        view: view.clone(),
        action_tx,
        controller: controller.clone(),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .ok();

    let tray_handle = if let Some(ref runtime) = rt {
        runtime.block_on(async { tray.spawn().await.ok() })
    } else {
        None
    };

    if tray_handle.is_some() {
        println!("[Tray] System tray icon registered (Standby: ○, Listening: ●).");
    } else {
        println!("[Tray] Notice: Tray host not detected or registration bypassed.");
    }

    LinuxPlatformHandle {
        is_listening,
        history_enabled,
        view,
        registry,
        tray_handle,
        rt,
        controller,
    }
}

/// Generate ARGB32 pixmap:
/// - Standby (false): Outline circle ○ (#E0E0E0)
/// - Listening (true): Filled solid circle ● (Vibrant Red #FF3B30)
fn create_circle_icon(filled: bool, size: i32) -> Icon {
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let center = size as f32 / 2.0;
    let radius = size as f32 * 0.38;
    let stroke_width = 2.5f32;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            let (a, r, g, b) = if filled {
                // Solid red circle ● (#FF3B30)
                if dist <= radius {
                    (255u8, 255u8, 59u8, 48u8)
                } else if dist < radius + 1.0 {
                    let alpha = ((1.0 - (dist - radius)) * 255.0) as u8;
                    (alpha, 255, 59, 48)
                } else {
                    (0, 0, 0, 0)
                }
            } else {
                // Outline circle ○ (#E0E0E0)
                let diff = (dist - radius).abs();
                if diff <= stroke_width / 2.0 {
                    (230u8, 220u8, 220u8, 220u8)
                } else if diff < stroke_width / 2.0 + 1.0 {
                    let alpha = ((1.0 - (diff - stroke_width / 2.0)) * 230.0) as u8;
                    (alpha, 220, 220, 220)
                } else {
                    (0, 0, 0, 0)
                }
            };

            data.push(a);
            data.push(r);
            data.push(g);
            data.push(b);
        }
    }

    Icon {
        width: size,
        height: size,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tray(view: PlatformView) -> (LinuxTray, crossbeam_channel::Receiver<AppAction>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let controller = Arc::new(Mutex::new(DesktopPanelController::new(tx.clone())));
        let tray = LinuxTray {
            is_listening: Arc::new(AtomicBool::new(false)),
            history_enabled: Arc::new(AtomicBool::new(false)),
            view: Arc::new(Mutex::new(view)),
            action_tx: tx,
            controller,
        };
        (tray, rx)
    }

    #[test]
    fn fallback_menu_contains_expected_items() {
        let registry = ModelRegistry::from_str(include_str!("../../../models/registry.json"))
            .expect("registry parses");
        let view = initial_platform_view(&registry);
        let (tray, _rx) = make_tray(view);
        let items = tray.menu();

        let labels: Vec<String> = items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Standard(s) => Some(s.label.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(
            labels,
            vec![
                "Open Echolet",
                "Start Listening (F10)",
                "Open History Folder",
                "Quit"
            ]
        );
    }

    #[test]
    fn fallback_menu_emits_actions() {
        let registry = ModelRegistry::from_str(include_str!("../../../models/registry.json"))
            .expect("registry parses");
        let view = initial_platform_view(&registry);
        let (mut tray, rx) = make_tray(view);
        let items = tray.menu();

        // Start listening
        if let MenuItem::Standard(s) = &items[1] {
            (s.activate)(&mut tray);
            match rx.try_recv().expect("action emitted") {
                AppAction::ToggleListening => {}
                other => panic!("expected ToggleListening, got {:?}", other),
            }
        } else {
            panic!("expected standard item at index 1");
        }

        // Open history folder
        if let MenuItem::Standard(s) = &items[3] {
            (s.activate)(&mut tray);
            match rx.try_recv().expect("action emitted") {
                AppAction::OpenHistoryFolder => {}
                other => panic!("expected OpenHistoryFolder, got {:?}", other),
            }
        } else {
            panic!("expected standard item at index 3");
        }

        // Quit
        if let MenuItem::Standard(s) = &items[5] {
            (s.activate)(&mut tray);
            match rx.try_recv().expect("action emitted") {
                AppAction::Quit => {}
                other => panic!("expected Quit, got {:?}", other),
            }
        } else {
            panic!("expected standard item at index 5");
        }
    }

    #[test]
    fn startup_projection_is_neutral() {
        let registry = ModelRegistry::from_str(include_str!("../../../models/registry.json"))
            .expect("registry parses");
        let view = initial_platform_view(&registry);
        assert_eq!(view.runtime_state, RuntimeState::NoModel);
        assert!(view.all_models().all(|m| !m.installed));
        assert!(view.all_models().all(|m| !m.selected));
        assert!(view.selected_model().is_none());
    }
}
