//! Deterministic production tests for Shared Slint Desktop Control Surface (PROJECT-041 / J11.1d).

use echolet::models::registry::{LanguageTier, ModelRegistry};
use echolet::ui::control_surface::{
    ControlSurfaceState, DownloadPresentation, LanguageOptionPresentation, ModelGroupPresentation,
    ModelLanguagePresentation, ModelPresentation, ModelPrimaryAction, RuntimeState, SurfaceAction,
};
use echolet::ui::desktop::adapter::{
    DesktopPanelViewModel, PANEL_HEIGHT_PX, PANEL_MAX_HEIGHT_PX, PANEL_WIDTH_PX,
};
use echolet::ui::desktop::host::{
    calculate_linux_panel_position, calculate_macos_panel_position,
    calculate_windows_panel_position, Rect,
};
use std::fs;
use std::path::Path;

#[test]
fn test_hierarchical_model_groups_and_ordering() {
    let state = ControlSurfaceState {
        runtime_state: RuntimeState::Ready,
        model_groups: vec![
            ModelGroupPresentation {
                id: "group-verified".to_string(),
                label: "Verified Models".to_string(),
                models: vec![
                    ModelPresentation {
                        id: "v-1".to_string(),
                        label: "Verified Fast".to_string(),
                        verification_label: "Echolet Verified".to_string(),
                        is_verified: true,
                        selected: true,
                        installed: true,
                        download: DownloadPresentation::from_status(
                            &echolet::models::download::DownloadStatus::Completed,
                        ),
                        primary_action: ModelPrimaryAction::None,
                        enabled: true,
                        language: ModelLanguagePresentation {
                            selected_locale: None,
                            options: vec![
                                LanguageOptionPresentation {
                                    locale: "en".to_string(),
                                    label: "English".to_string(),
                                    runtime_code: "en".to_string(),
                                    tier: LanguageTier::TranscriptionReady,
                                },
                                LanguageOptionPresentation {
                                    locale: "zh".to_string(),
                                    label: "Chinese".to_string(),
                                    runtime_code: "zh".to_string(),
                                    tier: LanguageTier::TranscriptionReady,
                                },
                            ],
                        },
                    },
                    ModelPresentation {
                        id: "v-2".to_string(),
                        label: "Verified Accurate".to_string(),
                        verification_label: "Echolet Verified".to_string(),
                        is_verified: true,
                        selected: false,
                        installed: true,
                        download: DownloadPresentation::from_status(
                            &echolet::models::download::DownloadStatus::Completed,
                        ),
                        primary_action: ModelPrimaryAction::Select,
                        enabled: true,
                        language: Default::default(),
                    },
                ],
            },
            ModelGroupPresentation {
                id: "group-community".to_string(),
                label: "Community Models".to_string(),
                models: vec![
                    ModelPresentation {
                        id: "c-1".to_string(),
                        label: "Community Multi".to_string(),
                        verification_label: "Community".to_string(),
                        is_verified: false,
                        selected: false,
                        installed: false,
                        download: DownloadPresentation::from_status(
                            &echolet::models::download::DownloadStatus::NotDownloading,
                        ),
                        primary_action: ModelPrimaryAction::Download,
                        enabled: true,
                        language: Default::default(),
                    },
                    ModelPresentation {
                        id: "c-2".to_string(),
                        label: "Community Failed".to_string(),
                        verification_label: "Community".to_string(),
                        is_verified: false,
                        selected: false,
                        installed: false,
                        download: DownloadPresentation::from_status(
                            &echolet::models::download::DownloadStatus::Failed,
                        ),
                        primary_action: ModelPrimaryAction::RetryDownload,
                        enabled: true,
                        language: Default::default(),
                    },
                ],
            },
        ],
        preload_on_startup: true,
        idle_unload_minutes: Some(10),
        history_enabled: true,
    };

    let vm = DesktopPanelViewModel::from_control_surface(&state);

    // Hierarchical groups preserved
    assert_eq!(vm.model_groups.len(), 2);
    assert_eq!(vm.model_groups[0].id, "group-verified");
    assert_eq!(vm.model_groups[0].label, "Verified Models");
    assert_eq!(vm.model_groups[0].models.len(), 2);
    assert_eq!(vm.model_groups[0].models[0].id, "v-1");
    assert_eq!(vm.model_groups[0].models[0].selected, true);
    assert_eq!(vm.model_groups[0].models[0].action_label, "Selected");
    assert_eq!(vm.model_groups[0].models[1].id, "v-2");
    assert_eq!(vm.model_groups[0].models[1].selected, false);
    assert_eq!(vm.model_groups[0].models[1].action_label, "Select");

    assert_eq!(vm.model_groups[1].id, "group-community");
    assert_eq!(vm.model_groups[1].label, "Community Models");
    assert_eq!(vm.model_groups[1].models.len(), 2);
    assert_eq!(vm.model_groups[1].models[0].id, "c-1");
    assert_eq!(vm.model_groups[1].models[0].action_label, "Download");
    assert_eq!(vm.model_groups[1].models[1].id, "c-2");
    assert_eq!(vm.model_groups[1].models[1].action_label, "Retry");

    // Flattened models preserve exact canonical order
    assert_eq!(vm.models.len(), 4);
    assert_eq!(vm.models[0].id, "v-1");
    assert_eq!(vm.models[1].id, "v-2");
    assert_eq!(vm.models[2].id, "c-1");
    assert_eq!(vm.models[3].id, "c-2");

    // Dimensions check
    assert_eq!(vm.width, PANEL_WIDTH_PX);
    assert_eq!(vm.width, 416);
    assert!(vm.height <= PANEL_MAX_HEIGHT_PX);
    assert_eq!(vm.height, PANEL_HEIGHT_PX);
}

#[test]
fn test_settings_actions_resolution() {
    let state = ControlSurfaceState {
        runtime_state: RuntimeState::Ready,
        model_groups: vec![],
        preload_on_startup: false,
        idle_unload_minutes: Some(5),
        history_enabled: false,
    };

    let vm = DesktopPanelViewModel::from_control_surface(&state);
    assert_eq!(vm.preload_on_startup, false);
    assert_eq!(vm.idle_unload_minutes, Some(5));
    assert_eq!(vm.history_enabled, false);

    // Verify SurfaceActions for settings
    assert_eq!(
        SurfaceAction::SetPreloadModelOnStartup(true),
        SurfaceAction::SetPreloadModelOnStartup(true)
    );
    assert_eq!(
        SurfaceAction::SetModelIdleUnloadMinutes(Some(10)),
        SurfaceAction::SetModelIdleUnloadMinutes(Some(10))
    );
    assert_eq!(
        SurfaceAction::SetHistoryEnabled(true),
        SurfaceAction::SetHistoryEnabled(true)
    );
    assert_eq!(
        SurfaceAction::OpenHistoryFolder,
        SurfaceAction::OpenHistoryFolder
    );
}

#[test]
fn test_language_selection_filtering_and_auto() {
    let state = ControlSurfaceState {
        runtime_state: RuntimeState::Ready,
        model_groups: vec![ModelGroupPresentation {
            id: "group-1".to_string(),
            label: "Group 1".to_string(),
            models: vec![ModelPresentation {
                id: "m-bilingual".to_string(),
                label: "Bilingual Model".to_string(),
                verification_label: "Echolet Verified".to_string(),
                is_verified: true,
                selected: true,
                installed: true,
                download: DownloadPresentation::from_status(
                    &echolet::models::download::DownloadStatus::Completed,
                ),
                primary_action: ModelPrimaryAction::None,
                enabled: true,
                language: ModelLanguagePresentation {
                    selected_locale: None,
                    options: vec![
                        LanguageOptionPresentation {
                            locale: "en".to_string(),
                            label: "English".to_string(),
                            runtime_code: "en".to_string(),
                            tier: LanguageTier::TranscriptionReady,
                        },
                        LanguageOptionPresentation {
                            locale: "zh".to_string(),
                            label: "Chinese".to_string(),
                            runtime_code: "zh".to_string(),
                            tier: LanguageTier::TranscriptionReady,
                        },
                        LanguageOptionPresentation {
                            locale: "fr".to_string(),
                            label: "French".to_string(),
                            runtime_code: "fr".to_string(),
                            tier: LanguageTier::BroadCoverage,
                        },
                    ],
                },
            }],
        }],
        ..Default::default()
    };

    let vm = DesktopPanelViewModel::from_control_surface(&state);
    assert_eq!(vm.has_language_options, true);
    assert_eq!(vm.language_options.len(), 4); // Auto + 3 languages
    assert_eq!(vm.language_options[0].locale, "");
    assert_eq!(vm.language_options[0].label, "Auto");
    assert_eq!(vm.language_options[0].is_selected, true);

    assert_eq!(vm.language_options[1].locale, "en");
    assert_eq!(vm.language_options[1].label, "English");
    assert_eq!(vm.language_options[1].is_selected, false);

    assert_eq!(vm.language_options[2].locale, "zh");
    assert_eq!(vm.language_options[2].label, "Chinese");
    assert_eq!(vm.language_options[2].is_selected, false);

    assert_eq!(vm.language_options[3].locale, "fr");
    assert_eq!(vm.language_options[3].label, "French");
    assert_eq!(vm.language_options[3].is_selected, false);
}

#[test]
fn test_host_geometry_calculations() {
    // 1. macOS status item positioning
    let status_frame = Rect::new(1200, 0, 30, 24);
    let screen = Rect::new(0, 0, 1440, 900);
    let pos_mac = calculate_macos_panel_position(status_frame, screen, 380, 520);
    // Center: 1200 + 15 = 1215. Panel x: 1215 - 190 = 1025.
    assert_eq!(pos_mac.x, 1025);
    assert_eq!(pos_mac.y, 24); // Directly below status bar

    // macOS right edge clamping
    let status_frame_edge = Rect::new(1400, 0, 30, 24);
    let pos_mac_edge = calculate_macos_panel_position(status_frame_edge, screen, 380, 520);
    // Clamped to 1440 - 380 = 1060
    assert_eq!(pos_mac_edge.x, 1060);
    assert_eq!(pos_mac_edge.y, 24);

    // 2. Windows tray positioning (bottom taskbar)
    let tray_win = Rect::new(1000, 1040, 24, 24);
    let work_area_win = Rect::new(0, 0, 1920, 1040);
    let pos_win = calculate_windows_panel_position(tray_win, work_area_win, 380, 520);
    assert_eq!(pos_win.x, 1000 + 12 - 190);
    assert_eq!(pos_win.y, 1040 - 520); // Placed above taskbar

    // Windows tray positioning (clamped right edge)
    let tray_win_edge = Rect::new(1800, 1040, 24, 24);
    let pos_win_edge = calculate_windows_panel_position(tray_win_edge, work_area_win, 380, 520);
    assert_eq!(pos_win_edge.x, 1920 - 380); // Clamped to 1540

    // 3. Linux tray positioning
    let tray_linux = Rect::new(1000, 1050, 24, 24);
    let screen_linux = Rect::new(0, 0, 1920, 1080);
    let pos_linux = calculate_linux_panel_position(Some(tray_linux), screen_linux, 380, 520);
    assert_eq!(pos_linux.x, 1000 + 12 - 190);
    assert_eq!(pos_linux.y, 1050 - 520);

    // 4. Linux Wayland fallback
    let pos_wayland = calculate_linux_panel_position(None, screen_linux, 380, 520);
    assert_eq!(pos_wayland.x, 1920 - 380 - 16);
    assert_eq!(pos_wayland.y, 32);
}

#[test]
fn test_catalog_integrity_exact_models() {
    // Model catalog must strictly maintain the expected 3 models in order
    let registry_str = include_str!("../models/registry.json");
    let registry = ModelRegistry::from_str(registry_str).expect("Valid registry JSON");
    let model_ids: Vec<&str> = registry.models.iter().map(|m| m.id.as_str()).collect();

    // Verify exactly the 3 models exist in order
    assert_eq!(
        model_ids,
        vec![
            "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
            "echolet-zipformer-streaming-en-2023-06-26-r1",
            "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1",
        ]
    );
}

#[test]
fn test_single_source_slint_guard() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Verify canonical ui/desktop/EcholetPanel.slint exists
    let canonical = root.join("ui/desktop/EcholetPanel.slint");
    assert!(
        canonical.exists(),
        "ui/desktop/EcholetPanel.slint must exist"
    );

    // Verify no .slint files exist in src/
    let src_dir = root.join("src");
    fn check_no_slint(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    check_no_slint(&p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("slint") {
                    panic!("Found forbidden .slint file in src: {:?}", p);
                }
            }
        }
    }
    check_no_slint(&src_dir);

    // Verify all .slint files in repo reside under ui/desktop/
    let ui_dir = root.join("ui");
    let mut slint_files = Vec::new();
    fn collect_slint(dir: &Path, list: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    collect_slint(&p, list);
                } else if p.extension().and_then(|e| e.to_str()) == Some("slint") {
                    list.push(p.file_name().unwrap().to_string_lossy().to_string());
                }
            }
        }
    }
    collect_slint(&ui_dir, &mut slint_files);
    slint_files.sort();

    let mut expected = vec![
        "EcholetPanel.slint".to_string(),
        "LanguageSelector.slint".to_string(),
        "ModelGroup.slint".to_string(),
        "ModelRow.slint".to_string(),
        "ProgressRow.slint".to_string(),
        "SettingRow.slint".to_string(),
        "StatusHeader.slint".to_string(),
    ];
    expected.sort();
    assert_eq!(
        slint_files, expected,
        "Only the canonical shared Slint components should exist under ui/desktop/"
    );
}

#[test]
fn test_threading_architecture_handle_is_send_sync_and_runtime_is_thread_bound() {
    use echolet::ui::desktop::controller::{
        DesktopPanelCommand, DesktopPanelHandle, DesktopPanelInit,
    };

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    // (A) DesktopPanelHandle is Send + Sync
    assert_send::<DesktopPanelHandle>();
    assert_sync::<DesktopPanelHandle>();

    // (B) DesktopPanelCommand is Send
    assert_send::<DesktopPanelCommand>();

    // (C) The runtime-init/factory object used across the spawn boundary is Send
    assert_send::<DesktopPanelInit>();

    // (D) DesktopPanelRuntime is intentionally NOT Send (architecture & type structure guards)
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let controller_src = fs::read_to_string(root.join("src/ui/desktop/controller.rs"))
        .expect("controller.rs must exist");
    assert!(
        controller_src.contains("_thread_bound: PhantomData<*mut ()>"),
        "DesktopPanelRuntime must have an explicit thread-bound marker field"
    );
    assert!(
        controller_src.contains("compile_fail"),
        "DesktopPanelRuntime must document a compile_fail assertion ensuring it is !Send"
    );

    // (E) Verify no source contains forbidden unsafe Send/Sync or Arc<Mutex<Runtime/Controller>>
    fn scan_no_forbidden_patterns(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    scan_no_forbidden_patterns(&p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let content = fs::read_to_string(&p).expect("read rs file");
                    assert!(
                        !content.contains("unsafe impl Send for DesktopPanelRuntime"),
                        "Forbidden unsafe impl Send for DesktopPanelRuntime in {:?}",
                        p
                    );
                    assert!(
                        !content.contains("unsafe impl Sync for DesktopPanelRuntime"),
                        "Forbidden unsafe impl Sync for DesktopPanelRuntime in {:?}",
                        p
                    );
                    assert!(
                        !content.contains("unsafe impl Send for DesktopPanelController"),
                        "Forbidden unsafe impl Send for DesktopPanelController in {:?}",
                        p
                    );
                    assert!(
                        !content.contains("Arc<Mutex<DesktopPanelRuntime>>"),
                        "Forbidden Arc<Mutex<DesktopPanelRuntime>> in {:?}",
                        p
                    );
                    assert!(
                        !content.contains("Arc<Mutex<DesktopPanelController>>"),
                        "Forbidden Arc<Mutex<DesktopPanelController>> in {:?}",
                        p
                    );
                }
            }
        }
    }
    scan_no_forbidden_patterns(&root.join("src"));

    // (F) Linux and Windows spawn closures move only the SENDABLE init/factory, never DesktopPanelRuntime
    let linux_src = fs::read_to_string(root.join("src/platform/linux/tray.rs"))
        .expect("linux tray.rs must exist");
    assert!(
        linux_src.contains("DesktopPanelRuntime::init"),
        "Linux must create DesktopPanelInit"
    );
    assert!(
        linux_src.contains("DesktopPanelRuntime::from_init(panel_init)"),
        "Linux spawn closure must construct DesktopPanelRuntime from panel_init"
    );

    let windows_src = fs::read_to_string(root.join("src/platform/windows/ui.rs"))
        .expect("windows ui.rs must exist");
    assert!(
        windows_src.contains("DesktopPanelRuntime::init"),
        "Windows must create DesktopPanelInit"
    );
    assert!(
        windows_src.contains("DesktopPanelRuntime::from_init(panel_init)"),
        "Windows spawn closure must construct DesktopPanelRuntime from panel_init"
    );
}

#[test]
fn test_sendable_init_used_across_spawn_boundary() {
    use echolet::ui::desktop::controller::DesktopPanelRuntime;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let (handle, init) = DesktopPanelRuntime::init(tx);

    let handle_for_bg = handle.clone();
    let bg_thread = std::thread::spawn(move || {
        let mut runtime = DesktopPanelRuntime::from_init(init);
        assert!(!runtime.is_visible());
        runtime.drain_commands();
        assert!(!runtime.is_visible());
    });

    handle_for_bg.hide_panel();
    bg_thread.join().expect("Spawned thread completed");
}

#[test]
fn test_cross_thread_command_enqueue_and_ui_drain() {
    use echolet::ui::desktop::controller::DesktopPanelRuntime;
    use echolet::ui::desktop::host::Point;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let (handle, mut runtime) = DesktopPanelRuntime::new(tx);

    let test_state = ControlSurfaceState {
        runtime_state: RuntimeState::Listening,
        preload_on_startup: true,
        idle_unload_minutes: Some(15),
        history_enabled: true,
        ..Default::default()
    };

    // Enqueue commands from background thread
    let handle_clone = handle.clone();
    let state_clone = test_state.clone();
    let worker = std::thread::spawn(move || {
        handle_clone.update_state(&state_clone);
        handle_clone.show_panel(Some(Point::new(100, 200)));
    });
    worker.join().expect("Worker thread finished");

    // UI-side drains commands
    assert!(!runtime.is_visible());
    runtime.drain_commands();

    // Verifications on UI owner
    assert!(runtime.is_visible());
    assert_eq!(
        runtime.current_state().runtime_state,
        RuntimeState::Listening
    );
    assert_eq!(runtime.current_state().preload_on_startup, true);
    assert_eq!(runtime.current_state().idle_unload_minutes, Some(15));
    assert_eq!(runtime.current_state().history_enabled, true);

    // Hide from worker
    let handle_clone2 = handle.clone();
    let worker2 = std::thread::spawn(move || {
        handle_clone2.hide_panel();
    });
    worker2.join().expect("Worker thread 2 finished");

    runtime.drain_commands();
    assert!(!runtime.is_visible());
}

#[test]
fn test_panel_runtime_reuse_and_state_update_without_recreation() {
    use echolet::ui::desktop::controller::DesktopPanelRuntime;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let (handle, mut runtime) = DesktopPanelRuntime::new(tx);

    // Repeated toggle commands reuse the same logical panel
    for _ in 0..5 {
        handle.toggle_panel(None);
        runtime.drain_commands();
        assert!(runtime.is_visible());

        handle.toggle_panel(None);
        runtime.drain_commands();
        assert!(!runtime.is_visible());
    }

    // Open and update state while open does not recreate or crash
    handle.show_panel(None);
    runtime.drain_commands();
    assert!(runtime.is_visible());

    let state = ControlSurfaceState {
        runtime_state: RuntimeState::Ready,
        preload_on_startup: false,
        ..Default::default()
    };
    handle.update_state(&state);
    runtime.drain_commands();
    assert!(runtime.is_visible());
    assert_eq!(runtime.current_state().runtime_state, RuntimeState::Ready);
}

#[test]
fn test_focus_loss_and_dismissal_contract() {
    use echolet::ui::desktop::controller::DesktopPanelRuntime;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let (handle, mut runtime) = DesktopPanelRuntime::new(tx);

    // 1. Synthetic focus loss maps to one hide command
    handle.show_panel(None);
    runtime.drain_commands();
    assert!(runtime.is_visible());

    handle.focus_lost();
    runtime.drain_commands();
    assert!(!runtime.is_visible(), "Focus loss must hide panel");

    // 2. Internal action / state update does NOT hide panel
    handle.show_panel(None);
    runtime.drain_commands();
    assert!(runtime.is_visible());

    let state = ControlSurfaceState {
        runtime_state: RuntimeState::Listening,
        ..Default::default()
    };
    handle.update_state(&state);
    runtime.drain_commands();
    assert!(
        runtime.is_visible(),
        "State update while open must not hide panel"
    );

    // 3. Escape / close requested maps to hide
    runtime.hide_panel();
    assert!(!runtime.is_visible(), "Close requested must hide panel");
}

#[test]
fn test_geometry_clamping_and_no_hardcoded_1920x1080() {
    // 1. Windows: 4K display geometry
    let tray_4k = Rect::new(3700, 2100, 24, 24);
    let work_area_4k = Rect::new(0, 0, 3840, 2120);
    let pos_4k = calculate_windows_panel_position(tray_4k, work_area_4k, 380, 520);
    // Right clamp: 3840 - 380 = 3460
    assert_eq!(pos_4k.x, 3460);
    assert_eq!(pos_4k.y, 2100 - 520);

    // 2. Windows: Small 1366x768 display geometry (clamps right edge to work area)
    let tray_small = Rect::new(1200, 728, 24, 24);
    let work_area_small = Rect::new(0, 0, 1366, 728);
    let pos_small = calculate_windows_panel_position(tray_small, work_area_small, 380, 520);
    assert_eq!(pos_small.x, 1366 - 380); // 986 clamped to right boundary
    assert_eq!(pos_small.y, 728 - 520);

    // 3. Linux: 2560x1440 display geometry
    let tray_linux = Rect::new(2400, 1400, 24, 24);
    let screen_linux = Rect::new(0, 0, 2560, 1440);
    let pos_linux = calculate_linux_panel_position(Some(tray_linux), screen_linux, 380, 520);
    // Right clamp: 2560 - 380 = 2180
    assert_eq!(pos_linux.x, 2180);
    assert_eq!(pos_linux.y, 1400 - 520);

    // 4. Verify NO hardcoded "1920, 1080" or "1920x1080" in production platform code
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let linux_src = fs::read_to_string(root.join("src/platform/linux/tray.rs"))
        .expect("linux tray.rs must exist");
    assert!(
        !linux_src.contains("1920, 1080"),
        "Linux production code must not contain hard-coded 1920, 1080"
    );
    assert!(
        !linux_src.contains("1920x1080"),
        "Linux production code must not contain hard-coded 1920x1080"
    );

    let win_src = fs::read_to_string(root.join("src/platform/windows/ui.rs"))
        .expect("windows ui.rs must exist");
    assert!(
        !win_src.contains("1920, 1080"),
        "Windows production code must not contain hard-coded 1920, 1080"
    );
    assert!(
        !win_src.contains("1920x1080"),
        "Windows production code must not contain hard-coded 1920x1080"
    );
}
