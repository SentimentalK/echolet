use crate::actions::AppAction;
use crate::models::ModelRegistry;
use crate::paths;
use crate::platform::PlatformHandle;
use crossbeam_channel::Sender;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use ksni::{Icon, Tray, TrayMethods};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ModelItemInfo {
    pub id: String,
    pub label: String,
    pub is_selected: bool,
    pub is_installed: bool,
    pub is_downloading: bool,
}

pub struct LinuxTray {
    pub is_listening: Arc<AtomicBool>,
    pub models_info: Arc<Mutex<Vec<ModelItemInfo>>>,
    pub action_tx: Sender<AppAction>,
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

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let is_rec = self.is_listening.load(Ordering::SeqCst);
        let toggle_tx = self.action_tx.clone();
        let quit_tx = self.action_tx.clone();

        let first_label = if is_rec {
            "Stop Listening"
        } else {
            "Start Listening"
        };

        let mut menu_items: Vec<MenuItem<Self>> = Vec::new();

        // 1. Start/Stop Listening
        menu_items.push(
            StandardItem {
                label: first_label.into(),
                activate: Box::new(move |_| {
                    let _ = toggle_tx.send(AppAction::ToggleListening);
                }),
                ..Default::default()
            }
            .into(),
        );

        // 2. Model item (Single informational line when <= 1 model, dynamic submenu when > 1)
        let models = self.models_info.lock().unwrap().clone();
        if models.len() <= 1 {
            let active_label = models
                .iter()
                .find(|m| m.is_selected)
                .map(|m| m.label.clone())
                .or_else(|| models.first().map(|m| m.label.clone()))
                .unwrap_or_else(|| "Chinese + English (X-ASR / 480ms) — 2026".to_string());

            menu_items.push(
                StandardItem {
                    label: format!("Model: {}", active_label),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            let mut sub_items: Vec<MenuItem<Self>> = Vec::new();

            for m in models {
                let action_tx = self.action_tx.clone();
                let model_id = m.id.clone();
                let is_selected = m.is_selected;
                let is_downloading = m.is_downloading;

                let label = if is_downloading {
                    format!("{} — Downloading...", m.label)
                } else if is_selected {
                    format!("✓ {}", m.label)
                } else if m.is_installed {
                    m.label.clone()
                } else {
                    format!("{} — Download", m.label)
                };

                let item_enabled = !is_rec && !is_downloading && !is_selected;
                let m_id = model_id.clone();

                sub_items.push(
                    StandardItem {
                        label,
                        enabled: item_enabled,
                        activate: Box::new(move |_| {
                            let _ = action_tx.send(AppAction::SelectModel(m_id.clone()));
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
            }

            menu_items.push(
                SubMenu {
                    label: "Model".into(),
                    enabled: !is_rec,
                    submenu: sub_items,
                    ..Default::default()
                }
                .into(),
            );
        }

        // 3. Hotkey: F10
        menu_items.push(
            StandardItem {
                label: "Hotkey: F10".into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        // 4. Quit
        menu_items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(move |_| {
                    let _ = quit_tx.send(AppAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );

        menu_items
    }
}

pub struct LinuxPlatformHandle {
    pub is_listening: Arc<AtomicBool>,
    pub models_info: Arc<Mutex<Vec<ModelItemInfo>>>,
    pub registry: ModelRegistry,
    pub tray_handle: Option<ksni::Handle<LinuxTray>>,
    pub rt: Option<tokio::runtime::Runtime>,
}

impl PlatformHandle for LinuxPlatformHandle {
    fn set_listening(&self, listening: bool) {
        self.is_listening.store(listening, Ordering::SeqCst);
        if let Some(handle) = &self.tray_handle {
            if let Some(rt) = &self.rt {
                rt.block_on(async {
                    handle.update(|_| {}).await;
                });
            }
        }
    }

    fn shutdown(&self) {
        if let Some(handle) = &self.tray_handle {
            handle.shutdown();
        }
    }

    fn update_models(
        &self,
        active_id: &str,
        installed_ids: &[String],
        downloading_ids: &[String],
    ) {
        let mut new_info = Vec::new();
        for entry in &self.registry.models {
            let is_selected = entry.id == active_id;
            let is_installed = installed_ids.iter().any(|id| id == &entry.id);
            let is_downloading = downloading_ids.iter().any(|id| id == &entry.id);

            new_info.push(ModelItemInfo {
                id: entry.id.clone(),
                label: entry.display_title(),
                is_selected,
                is_installed,
                is_downloading,
            });
        }

        if let Ok(mut lock) = self.models_info.lock() {
            *lock = new_info;
        }

        if let Some(handle) = &self.tray_handle {
            if let Some(rt) = &self.rt {
                rt.block_on(async {
                    handle.update(|_| {}).await;
                });
            }
        }
    }
}

pub fn spawn_linux_tray(action_tx: Sender<AppAction>) -> LinuxPlatformHandle {
    let is_listening = Arc::new(AtomicBool::new(false));

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

    let mut initial_models = Vec::new();
    for entry in &registry.models {
        let is_selected = entry.id == registry.default_model_id;
        initial_models.push(ModelItemInfo {
            id: entry.id.clone(),
            label: entry.display_title(),
            is_selected,
            is_installed: is_selected, // Bundled default is initially installed
            is_downloading: false,
        });
    }

    let models_info = Arc::new(Mutex::new(initial_models));

    let tray = LinuxTray {
        is_listening: is_listening.clone(),
        models_info: models_info.clone(),
        action_tx,
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .ok();

    let tray_handle = if let Some(ref runtime) = rt {
        runtime.block_on(async {
            tray.spawn().await.ok()
        })
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
        models_info,
        registry,
        tray_handle,
        rt,
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
