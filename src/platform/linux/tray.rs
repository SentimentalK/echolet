use crate::actions::AppAction;
use crate::platform::PlatformHandle;
use crossbeam_channel::Sender;
use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, Tray, TrayMethods};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct LinuxTray {
    pub is_listening: Arc<AtomicBool>,
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

        vec![
            // Line 1: Start/Stop Listening
            StandardItem {
                label: first_label.into(),
                activate: Box::new(move |_| {
                    let _ = toggle_tx.send(AppAction::ToggleListening);
                }),
                ..Default::default()
            }
            .into(),
            // Line 2: Hotkey: F10 (disabled/informational)
            StandardItem {
                label: "Hotkey: F10".into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            // Line 3: Quit
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(move |_| {
                    let _ = quit_tx.send(AppAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct LinuxPlatformHandle {
    pub is_listening: Arc<AtomicBool>,
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
}

pub fn spawn_linux_tray(action_tx: Sender<AppAction>) -> LinuxPlatformHandle {
    let is_listening = Arc::new(AtomicBool::new(false));
    let tray = LinuxTray {
        is_listening: is_listening.clone(),
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
