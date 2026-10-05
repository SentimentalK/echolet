use crate::actions::AppAction;
use crate::config::EcholetConfig;
use crate::models::registry::{LanguageTier, ModelRegistry};
use crate::paths;
use crate::platform::view::{download_status_label, PlatformModelItem, PlatformView, RuntimeState};
use crate::platform::PlatformHandle;
use crossbeam_channel::Sender;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use ksni::{Icon, Tray, TrayMethods};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Accepted idle-unload policy choices, in menu order.
const IDLE_UNLOAD_OPTIONS: [(Option<u32>, &str); 6] = [
    (Some(0), "Immediate"),
    (Some(1), "1 minute"),
    (Some(5), "5 minutes"),
    (Some(10), "10 minutes (default)"),
    (Some(30), "30 minutes"),
    (None, "Never"),
];

pub struct LinuxTray {
    pub is_listening: Arc<AtomicBool>,
    pub history_enabled: Arc<AtomicBool>,
    pub view: Arc<Mutex<PlatformView>>,
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
        let view = self.view.lock().map(|v| v.clone()).unwrap_or_default();

        let mut menu_items: Vec<MenuItem<Self>> = Vec::new();

        // 1. Start/Stop Listening
        menu_items.push(
            StandardItem {
                label: if is_rec {
                    "Stop Listening"
                } else {
                    "Start Listening"
                }
                .into(),
                activate: {
                    let tx = self.action_tx.clone();
                    Box::new(move |_| {
                        let _ = tx.send(AppAction::ToggleListening);
                    })
                },
                ..Default::default()
            }
            .into(),
        );

        // 2. Model submenu (never assume a fixed number of models)
        menu_items.push(
            SubMenu {
                label: "Model".into(),
                enabled: true,
                submenu: build_model_submenu(&self.action_tx, &view, is_rec),
                ..Default::default()
            }
            .into(),
        );

        // 3. Language submenu, only for the active model that supports it.
        if let Some(active) = view.selected_model() {
            if !active.language.options.is_empty() {
                menu_items.push(
                    SubMenu {
                        label: "Language".into(),
                        enabled: !is_rec,
                        submenu: build_language_submenu(&self.action_tx, active, is_rec),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        // 4. Runtime status row (authoritative projection, disabled)
        menu_items.push(
            StandardItem {
                label: view.runtime_state.status_label().into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        // 5. Preload toggle
        menu_items.push(
            StandardItem {
                label: if view.preload_on_startup {
                    "✓ Preload Model on Startup".into()
                } else {
                    "Preload Model on Startup: Off".into()
                },
                activate: {
                    let tx = self.action_tx.clone();
                    let next = !view.preload_on_startup;
                    Box::new(move |_| {
                        let _ = tx.send(AppAction::SetPreloadModelOnStartup(next));
                    })
                },
                ..Default::default()
            }
            .into(),
        );

        // 6. Idle-unload policy selector
        menu_items.push(
            SubMenu {
                label: idle_unload_label(view.idle_unload_minutes),
                enabled: true,
                submenu: build_idle_submenu(&self.action_tx, view.idle_unload_minutes),
                ..Default::default()
            }
            .into(),
        );

        // 7. Local History section
        let hist_enabled = self.history_enabled.load(Ordering::SeqCst);
        let hist_toggle_tx = self.action_tx.clone();
        let hist_open_tx = self.action_tx.clone();

        if !hist_enabled {
            menu_items.push(
                StandardItem {
                    label: "Local History: Off".into(),
                    activate: Box::new(move |_| {
                        let _ = hist_toggle_tx.send(AppAction::ToggleHistory);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            let history_path_str = paths::history_dir()
                .to_string_lossy()
                .replace(&std::env::var("HOME").unwrap_or_default(), "~");

            menu_items.push(
                StandardItem {
                    label: "✓ Local History".into(),
                    activate: Box::new(move |_| {
                        let _ = hist_toggle_tx.send(AppAction::ToggleHistory);
                    }),
                    ..Default::default()
                }
                .into(),
            );

            menu_items.push(
                StandardItem {
                    label: "    Open History Folder".into(),
                    activate: Box::new(move |_| {
                        let _ = hist_open_tx.send(AppAction::OpenHistoryFolder);
                    }),
                    ..Default::default()
                }
                .into(),
            );

            menu_items.push(
                StandardItem {
                    label: format!("    {}", history_path_str),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }

        // 8. Hotkey: F10
        menu_items.push(
            StandardItem {
                label: "Hotkey: F10".into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        // 9. Quit
        menu_items.push(
            StandardItem {
                label: "Quit".into(),
                activate: {
                    let tx = self.action_tx.clone();
                    Box::new(move |_| {
                        let _ = tx.send(AppAction::Quit);
                    })
                },
                ..Default::default()
            }
            .into(),
        );

        menu_items
    }
}

/// Concise, unambiguous model menu label including verification + state.
fn model_item_label(m: &PlatformModelItem) -> String {
    let base = if m.is_selected {
        format!("✓ {} (Selected)", m.label)
    } else if let Some(dl) = download_status_label(&m.download) {
        format!("{} — {}", m.label, dl)
    } else if m.is_installed {
        format!("{} — Installed", m.label)
    } else {
        format!("{} — Download", m.label)
    };
    format!("{} · {}", base, m.verification_label)
}

fn build_model_submenu(
    tx: &Sender<AppAction>,
    view: &PlatformView,
    is_rec: bool,
) -> Vec<MenuItem<LinuxTray>> {
    let mut sub: Vec<MenuItem<LinuxTray>> = Vec::new();

    let any_installed = view.models.iter().any(|m| m.is_installed);
    if !any_installed && view.selected_model().is_none() {
        sub.push(
            StandardItem {
                label: "Model: None installed".into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );
    }

    for m in &view.models {
        let enabled = !is_rec && !m.download.is_in_progress() && !m.is_selected;
        let tx = tx.clone();
        let id = m.id.clone();
        let is_installed = m.is_installed;
        sub.push(
            StandardItem {
                label: model_item_label(m),
                enabled,
                activate: Box::new(move |_| {
                    // Download and selection are distinct actions: uninstalled
                    // models only ever start a download.
                    let action = if is_installed {
                        AppAction::SelectModel(id.clone())
                    } else {
                        AppAction::DownloadModel(id.clone())
                    };
                    let _ = tx.send(action);
                }),
                ..Default::default()
            }
            .into(),
        );
    }

    sub
}

fn build_language_submenu(
    tx: &Sender<AppAction>,
    active: &PlatformModelItem,
    is_rec: bool,
) -> Vec<MenuItem<LinuxTray>> {
    let mut sub: Vec<MenuItem<LinuxTray>> = Vec::new();

    let auto_checked = active.language.selected_locale.is_none();
    let auto_tx = tx.clone();
    let auto_id = active.id.clone();
    sub.push(
        StandardItem {
            label: if auto_checked { "✓ Auto" } else { "Auto" }.into(),
            enabled: !is_rec,
            activate: Box::new(move |_| {
                let _ = auto_tx.send(AppAction::SelectLanguage {
                    model_id: auto_id.clone(),
                    locale: None,
                });
            }),
            ..Default::default()
        }
        .into(),
    );

    sub.push(
        SubMenu {
            label: "Transcription-ready".into(),
            enabled: !is_rec,
            submenu: build_tier_submenu(tx, active, LanguageTier::TranscriptionReady, is_rec),
            ..Default::default()
        }
        .into(),
    );
    sub.push(
        SubMenu {
            label: "Broad coverage".into(),
            enabled: !is_rec,
            submenu: build_tier_submenu(tx, active, LanguageTier::BroadCoverage, is_rec),
            ..Default::default()
        }
        .into(),
    );

    sub
}

fn build_tier_submenu(
    tx: &Sender<AppAction>,
    active: &PlatformModelItem,
    tier: LanguageTier,
    is_rec: bool,
) -> Vec<MenuItem<LinuxTray>> {
    let mut sub = Vec::new();
    for opt in active.language.options.iter().filter(|o| o.tier == tier) {
        let checked = active.language.selected_locale.as_deref() == Some(opt.locale.as_str());
        let label = if checked {
            format!("✓ {}", opt.label)
        } else {
            opt.label.clone()
        };
        let tx = tx.clone();
        let model_id = active.id.clone();
        let locale = opt.locale.clone();
        sub.push(
            StandardItem {
                label,
                enabled: !is_rec,
                activate: Box::new(move |_| {
                    let _ = tx.send(AppAction::SelectLanguage {
                        model_id: model_id.clone(),
                        locale: Some(locale.clone()),
                    });
                }),
                ..Default::default()
            }
            .into(),
        );
    }
    sub
}

fn idle_unload_label(current: Option<u32>) -> String {
    let name = IDLE_UNLOAD_OPTIONS
        .iter()
        .find(|(value, _)| *value == current)
        .map(|(_, name)| *name)
        .unwrap_or("Custom");
    format!("Idle Unload: {}", name)
}

fn build_idle_submenu(tx: &Sender<AppAction>, current: Option<u32>) -> Vec<MenuItem<LinuxTray>> {
    IDLE_UNLOAD_OPTIONS
        .iter()
        .map(|(value, name)| {
            let checked = current == *value;
            let tx = tx.clone();
            let value = *value;
            StandardItem {
                label: if checked {
                    format!("✓ {}", name)
                } else {
                    (*name).to_string()
                },
                activate: Box::new(move |_| {
                    let _ = tx.send(AppAction::SetModelIdleUnloadMinutes(value));
                }),
                ..Default::default()
            }
            .into()
        })
        .collect()
}

pub struct LinuxPlatformHandle {
    pub is_listening: Arc<AtomicBool>,
    pub history_enabled: Arc<AtomicBool>,
    pub view: Arc<Mutex<PlatformView>>,
    pub registry: ModelRegistry,
    pub tray_handle: Option<ksni::Handle<LinuxTray>>,
    pub rt: Option<tokio::runtime::Runtime>,
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
        if let Some(handle) = &self.tray_handle {
            handle.shutdown();
        }
    }

    fn update_models(&self, view: &PlatformView) {
        if let Ok(mut lock) = self.view.lock() {
            *lock = view.clone();
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

/// Neutral starting projection: nothing is assumed installed or selected. The
/// app immediately replaces this with the manager's real state, so the tray can
/// never briefly lie about a model-free install.
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

    let tray = LinuxTray {
        is_listening: is_listening.clone(),
        history_enabled: history_enabled.clone(),
        view: view.clone(),
        action_tx,
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
    use crate::models::download::DownloadStatus;
    use crate::platform::view::{LanguageOptionView, ModelLanguageView, PlatformModelItem};

    fn sample_model(id: &str, label: &str) -> PlatformModelItem {
        PlatformModelItem {
            id: id.into(),
            label: label.into(),
            verification_label: "Echolet Verified".into(),
            is_verified: true,
            is_selected: false,
            is_installed: false,
            download: DownloadStatus::NotDownloading,
            language: ModelLanguageView::default(),
        }
    }

    fn collect_standard(items: &[MenuItem<LinuxTray>], out: &mut Vec<(String, bool)>) {
        for item in items {
            match item {
                MenuItem::Standard(s) => out.push((s.label.clone(), s.enabled)),
                MenuItem::SubMenu(sub) => collect_standard(&sub.submenu, out),
                _ => {}
            }
        }
    }

    /// Collects every row including submenu headers (which are not activated).
    fn collect_all_labels(items: &[MenuItem<LinuxTray>], out: &mut Vec<String>) {
        for item in items {
            match item {
                MenuItem::Standard(s) => out.push(s.label.clone()),
                MenuItem::SubMenu(sub) => {
                    out.push(sub.label.clone());
                    collect_all_labels(&sub.submenu, out);
                }
                _ => {}
            }
        }
    }

    fn find_item<'a>(
        items: &'a [MenuItem<LinuxTray>],
        predicate: impl Fn(&str) -> bool + Copy,
    ) -> Option<&'a StandardItem<LinuxTray>> {
        for item in items {
            match item {
                MenuItem::Standard(s) if predicate(&s.label) => return Some(s),
                MenuItem::SubMenu(sub) => {
                    if let Some(found) = find_item(&sub.submenu, predicate) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn make_tray(view: PlatformView) -> (LinuxTray, crossbeam_channel::Receiver<AppAction>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let tray = LinuxTray {
            is_listening: Arc::new(AtomicBool::new(false)),
            history_enabled: Arc::new(AtomicBool::new(false)),
            view: Arc::new(Mutex::new(view)),
            action_tx: tx,
        };
        (tray, rx)
    }

    #[test]
    fn uninstalled_model_is_actionable_download_and_does_not_select() {
        let mut view = PlatformView::default();
        let mut m = sample_model("m1", "Model One");
        m.download = DownloadStatus::NotDownloading;
        view.models.push(m);
        let (mut tray, rx) = make_tray(view);
        let items = tray.menu();
        let item = find_item(&items, |l| {
            l.contains("Model One") && l.contains("— Download")
        })
        .expect("downloadable model row");
        assert!(item.enabled);
        (item.activate)(&mut tray);
        match rx.try_recv().expect("action emitted") {
            AppAction::DownloadModel(id) => assert_eq!(id, "m1"),
            other => panic!("expected DownloadModel, got {:?}", other),
        }
    }

    #[test]
    fn installed_model_row_selects_not_downloads() {
        let mut view = PlatformView::default();
        let mut m = sample_model("m1", "Model One");
        m.is_installed = true;
        view.models.push(m);
        let (mut tray, rx) = make_tray(view);
        let items = tray.menu();
        let item = find_item(&items, |l| l.contains("Installed")).expect("installed row");
        assert!(item.enabled);
        (item.activate)(&mut tray);
        match rx.try_recv().expect("action emitted") {
            AppAction::SelectModel(id) => assert_eq!(id, "m1"),
            other => panic!("expected SelectModel, got {:?}", other),
        }
    }

    #[test]
    fn selected_and_downloading_rows_are_disabled() {
        let mut view = PlatformView::default();
        let mut selected = sample_model("m1", "Selected Model");
        selected.is_selected = true;
        selected.is_installed = true;
        let mut downloading = sample_model("m2", "Downloading Model");
        downloading.download = DownloadStatus::Downloading {
            downloaded_bytes: 50,
            total_bytes: Some(100),
        };
        view.models.push(selected);
        view.models.push(downloading);
        let (tray, _rx) = make_tray(view);
        let items = tray.menu();
        let mut labels = Vec::new();
        collect_standard(&items, &mut labels);
        assert!(labels.iter().any(|(l, e)| l.contains("(Selected)") && !*e));
        assert!(labels
            .iter()
            .any(|(l, e)| l.contains("Downloading 50%") && !*e));
    }

    #[test]
    fn language_menu_has_auto_plus_32_and_tier_groups() {
        let registry = ModelRegistry::from_str(include_str!("../../../models/registry.json"))
            .expect("registry parses");
        let config = EcholetConfig::default();
        let mut installed = HashSet::new();
        let nemotron = "echolet-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11-r1";
        installed.insert(nemotron.to_string());
        let view = crate::platform::build_view(
            &registry,
            Some(nemotron),
            &installed,
            &HashSet::new(),
            &HashMap::new(),
            &config,
            RuntimeState::Unloaded,
        );

        let active = view
            .selected_model()
            .cloned()
            .expect("active model selected");
        let (tray, _rx) = make_tray(view);
        let items = tray.menu();

        // Auto + exactly the 32 selectable locales as leaf rows.
        let mut labels = Vec::new();
        collect_standard(&items, &mut labels);
        assert!(labels.iter().any(|(l, _)| l.starts_with("✓ Auto")));
        let locale_rows = labels
            .iter()
            .filter(|(l, _)| {
                l.contains("Japanese (Japan)")
                    || l.contains("Mandarin Chinese (China)")
                    || l.contains("Greek (Greece)")
            })
            .collect::<Vec<_>>();
        // Japanese + Chinese present; adaptation-ready Greek must be absent.
        assert!(locale_rows.iter().any(|(l, _)| l.contains("Japanese")));
        assert!(locale_rows.iter().any(|(l, _)| l.contains("Mandarin")));
        assert!(!locale_rows.iter().any(|(l, _)| l.contains("Greek")));

        let lang = build_language_submenu(&tray.action_tx, &active, false);
        let mut lang_labels = Vec::new();
        collect_standard(&lang, &mut lang_labels);
        // Auto + 32 locales.
        assert_eq!(lang_labels.len(), 33);
        let tr = build_tier_submenu(
            &tray.action_tx,
            &active,
            LanguageTier::TranscriptionReady,
            false,
        );
        let bc = build_tier_submenu(&tray.action_tx, &active, LanguageTier::BroadCoverage, false);
        let mut tr_labels = Vec::new();
        collect_standard(&tr, &mut tr_labels);
        let mut bc_labels = Vec::new();
        collect_standard(&bc, &mut bc_labels);
        assert_eq!(tr_labels.len(), 19);
        assert_eq!(bc_labels.len(), 13);
        // Adaptation-ready never appears.
        for (l, _) in lang_labels.iter() {
            assert!(!l.contains("Greek") && !l.contains("Thai") && !l.contains("Hebrew"));
        }
    }

    #[test]
    fn runtime_status_and_settings_rows_render() {
        let registry = ModelRegistry::from_str(include_str!("../../../models/registry.json"))
            .expect("registry parses");
        let mut config = EcholetConfig::default();
        config.preload_model_on_startup = true;
        config.model_idle_unload_minutes = Some(5);
        let view = crate::platform::build_view(
            &registry,
            None,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            &config,
            RuntimeState::Loading,
        );
        let (tray, _rx) = make_tray(view);
        let items = tray.menu();
        let mut labels = Vec::new();
        collect_standard(&items, &mut labels);
        assert!(labels.iter().any(|(l, _)| l == "Status: Loading model…"));
        assert!(labels
            .iter()
            .any(|(l, _)| l.contains("✓ Preload Model on Startup")));
        let mut all_labels = Vec::new();
        collect_all_labels(&items, &mut all_labels);
        assert!(all_labels.iter().any(|l| l == "Idle Unload: 5 minutes"));
        assert!(labels
            .iter()
            .any(|(l, _)| l.contains("5 minutes") && l.starts_with("✓")));
    }

    #[test]
    fn startup_projection_is_neutral() {
        let registry = ModelRegistry::from_str(include_str!("../../../models/registry.json"))
            .expect("registry parses");
        let view = initial_platform_view(&registry);
        assert_eq!(view.runtime_state, RuntimeState::NoModel);
        assert!(view.models.iter().all(|m| !m.is_installed));
        assert!(view.models.iter().all(|m| !m.is_selected));
        assert!(view.selected_model().is_none());
        assert!(view
            .models
            .iter()
            .all(|m| m.download == DownloadStatus::NotDownloading));
    }

    #[test]
    fn language_option_lookup_supports_tier_grouping() {
        // Guards the view type used by the menu: tier filters are exact.
        let view = ModelLanguageView {
            options: vec![
                LanguageOptionView {
                    locale: "ja-JP".into(),
                    label: "Japanese (Japan)".into(),
                    runtime_code: "ja".into(),
                    tier: LanguageTier::TranscriptionReady,
                },
                LanguageOptionView {
                    locale: "zh-CN".into(),
                    label: "Mandarin Chinese (China)".into(),
                    runtime_code: "zh".into(),
                    tier: LanguageTier::BroadCoverage,
                },
            ],
            selected_locale: Some("ja-JP".into()),
        };
        assert_eq!(view.transcription_ready().count(), 1);
        assert_eq!(view.broad_coverage().count(), 1);
    }
}
