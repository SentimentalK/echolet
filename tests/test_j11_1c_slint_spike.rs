//! Deterministic tests for Slint UI Spike (PROJECT-041 / J11.1c).

#[cfg(feature = "slint-ui-spike")]
mod slint_spike_tests {
    use echolet::ui::control_surface::{
        ControlSurfaceState, DownloadPresentation, ModelGroupPresentation,
        ModelPresentation, ModelPrimaryAction, RuntimeState, SurfaceAction,
    };
    use echolet::ui::desktop::{
        DesktopPanelViewModel, SlintControlSurfaceAdapter, PANEL_HEIGHT_PX, PANEL_WIDTH_PX,
    };
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_group_ordering_and_model_ordering_preserved() {
        let state = ControlSurfaceState {
            runtime_state: RuntimeState::Ready,
            model_groups: vec![
                ModelGroupPresentation {
                    id: "group-1-zh-en".to_string(),
                    label: "Bilingual Models".to_string(),
                    models: vec![
                        ModelPresentation {
                            id: "m-1a".to_string(),
                            label: "Model 1A".to_string(),
                            verification_label: "Echolet Verified".to_string(),
                            is_verified: true,
                            selected: true,
                            installed: true,
                            download: DownloadPresentation::from_status(
                                &echolet::models::download::DownloadStatus::Completed,
                            ),
                            primary_action: ModelPrimaryAction::None,
                            enabled: true,
                            language: Default::default(),
                        },
                        ModelPresentation {
                            id: "m-1b".to_string(),
                            label: "Model 1B".to_string(),
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
                    id: "group-2-multilingual".to_string(),
                    label: "Multilingual Models".to_string(),
                    models: vec![ModelPresentation {
                        id: "m-2a".to_string(),
                        label: "Model 2A".to_string(),
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
                    }],
                },
            ],
            preload_on_startup: true,
            idle_unload_minutes: Some(15),
            history_enabled: true,
        };

        let vm = DesktopPanelViewModel::from_control_surface(&state);

        // Group ordering preserved
        assert_eq!(vm.group_label, "Bilingual Models");
        assert_eq!(vm.models.len(), 3);

        // Sequence of flattened models matches input order exactly
        assert_eq!(vm.models[0].id, "m-1a");
        assert_eq!(vm.models[0].group_id, "group-1-zh-en");
        assert_eq!(vm.models[0].group_label, "Bilingual Models");

        assert_eq!(vm.models[1].id, "m-1b");
        assert_eq!(vm.models[1].group_id, "group-1-zh-en");

        assert_eq!(vm.models[2].id, "m-2a");
        assert_eq!(vm.models[2].group_id, "group-2-multilingual");
        assert_eq!(vm.models[2].group_label, "Multilingual Models");

        // Settings typed bindings preserved
        assert_eq!(vm.preload_on_startup, true);
        assert_eq!(vm.idle_unload_minutes, Some(15));
        assert_eq!(vm.history_enabled, true);
    }

    #[test]
    fn test_model_primary_action_passed_through_not_rederived() {
        let mut model_select = ModelPresentation {
            id: "model-sel".to_string(),
            label: "Model Select".to_string(),
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
        };

        let model_dl = ModelPresentation {
            id: "model-dl".to_string(),
            label: "Model Download".to_string(),
            verification_label: "Echolet Verified".to_string(),
            is_verified: true,
            selected: false,
            installed: false,
            download: DownloadPresentation::from_status(
                &echolet::models::download::DownloadStatus::NotDownloading,
            ),
            primary_action: ModelPrimaryAction::Download,
            enabled: true,
            language: Default::default(),
        };

        let model_retry = ModelPresentation {
            id: "model-retry".to_string(),
            label: "Model Retry".to_string(),
            verification_label: "Echolet Verified".to_string(),
            is_verified: true,
            selected: false,
            installed: false,
            download: DownloadPresentation::from_status(
                &echolet::models::download::DownloadStatus::Failed,
            ),
            primary_action: ModelPrimaryAction::RetryDownload,
            enabled: true,
            language: Default::default(),
        };

        let model_none = ModelPresentation {
            id: "model-none".to_string(),
            label: "Model Active".to_string(),
            verification_label: "Echolet Verified".to_string(),
            is_verified: true,
            selected: true,
            installed: true,
            download: DownloadPresentation::from_status(
                &echolet::models::download::DownloadStatus::Completed,
            ),
            primary_action: ModelPrimaryAction::None,
            enabled: true,
            language: Default::default(),
        };

        let state = ControlSurfaceState {
            runtime_state: RuntimeState::Ready,
            model_groups: vec![ModelGroupPresentation {
                id: "g".to_string(),
                label: "Group".to_string(),
                models: vec![
                    model_select.clone(),
                    model_dl.clone(),
                    model_retry.clone(),
                    model_none.clone(),
                ],
            }],
            ..Default::default()
        };

        let vm = DesktopPanelViewModel::from_control_surface(&state);

        // Action labels reflect primary_action
        assert_eq!(vm.models[0].action_label, "Select");
        assert_eq!(vm.models[1].action_label, "Download");
        assert_eq!(vm.models[2].action_label, "Retry");
        assert_eq!(vm.models[3].action_label, "Selected");

        // Action resolution directly emits canonical SurfaceAction without re-inferring
        assert_eq!(
            vm.resolve_surface_action("model-sel"),
            Some(SurfaceAction::SelectModel("model-sel".to_string()))
        );
        assert_eq!(
            vm.resolve_surface_action("model-dl"),
            Some(SurfaceAction::DownloadModel("model-dl".to_string()))
        );
        assert_eq!(
            vm.resolve_surface_action("model-retry"),
            Some(SurfaceAction::DownloadModel("model-retry".to_string()))
        );
        assert_eq!(vm.resolve_surface_action("model-none"), None);

        // Disabled row returns None regardless of action
        model_select.enabled = false;
        let state_disabled = ControlSurfaceState {
            runtime_state: RuntimeState::Ready,
            model_groups: vec![ModelGroupPresentation {
                id: "g".to_string(),
                label: "Group".to_string(),
                models: vec![model_select],
            }],
            ..Default::default()
        };
        let vm_disabled = DesktopPanelViewModel::from_control_surface(&state_disabled);
        assert_eq!(vm_disabled.models[0].action_enabled, false);
        assert_eq!(vm_disabled.resolve_surface_action("model-sel"), None);
    }

    #[test]
    fn test_fixed_panel_dimensions_independent_of_content_length() {
        let fixture_normal = SlintControlSurfaceAdapter::create_spike_fixture();
        let vm_normal = DesktopPanelViewModel::from_control_surface(&fixture_normal);

        let fixture_long = SlintControlSurfaceAdapter::create_spike_fixture_long_names();
        let vm_long = DesktopPanelViewModel::from_control_surface(&fixture_long);

        assert_eq!(vm_normal.width, 416);
        assert_eq!(vm_normal.height, 816);
        assert_eq!(PANEL_WIDTH_PX, 416);
        assert_eq!(PANEL_HEIGHT_PX, 816);
        assert!(PANEL_HEIGHT_PX <= 816);

        // Even with long name inputs, panel constants and view model dimensions remain invariant
        assert_eq!(vm_long.width, 416);
        assert_eq!(vm_long.height, 816);
        assert_eq!(vm_long.width, vm_normal.width);
        assert_eq!(vm_long.height, vm_normal.height);
    }

    #[test]
    fn test_runtime_status_binding_updates() {
        for (runtime_state, expected_status) in [
            (RuntimeState::NoModel, "Status: No model"),
            (RuntimeState::Unloaded, "Status: Unloaded"),
            (RuntimeState::Loading, "Status: Loading model…"),
            (RuntimeState::Ready, "Status: Ready"),
            (RuntimeState::Listening, "Status: Listening"),
        ] {
            let state = ControlSurfaceState {
                runtime_state,
                model_groups: Vec::new(),
                ..Default::default()
            };
            let vm = DesktopPanelViewModel::from_control_surface(&state);
            assert_eq!(vm.status_text, expected_status);
        }
    }

    #[test]
    fn test_diagnostic_callback_round_trip() {
        let state = SlintControlSurfaceAdapter::create_spike_fixture();
        let mut vm = DesktopPanelViewModel::from_control_surface(&state);

        assert_eq!(vm.diagnostic_state, "Initial");
        assert_eq!(vm.diagnostic_count, 0);

        // First click
        vm.trigger_diagnostic_toggle();
        assert_eq!(vm.diagnostic_state, "Active");
        assert_eq!(vm.diagnostic_count, 1);

        // Second click
        vm.trigger_diagnostic_toggle();
        assert_eq!(vm.diagnostic_state, "Toggled");
        assert_eq!(vm.diagnostic_count, 2);

        // Third click
        vm.trigger_diagnostic_toggle();
        assert_eq!(vm.diagnostic_state, "Active");
        assert_eq!(vm.diagnostic_count, 3);

        // Verification: original control surface state was not mutated
        assert_eq!(state.runtime_state, RuntimeState::Ready);
    }

    #[test]
    fn test_no_network_or_download_required_for_spike_startup() {
        let fixture = SlintControlSurfaceAdapter::create_spike_fixture();
        assert!(!fixture.model_groups.is_empty());
        let vm = DesktopPanelViewModel::from_control_surface(&fixture);
        assert_eq!(vm.models.len(), 2);
        assert_eq!(vm.status_text, "Status: Ready");
    }

    #[test]
    fn test_shared_component_single_source_guard() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));

        // Exactly one EcholetPanel.slint must exist in the entire repo
        let canonical_slint = root.join("ui/desktop/EcholetPanel.slint");
        assert!(
            canonical_slint.exists(),
            "Canonical ui/desktop/EcholetPanel.slint must exist"
        );

        // Verify no .slint files exist in src/platform/
        let platform_dir = root.join("src/platform");
        if platform_dir.exists() {
            fn check_no_slint(dir: &Path) {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            check_no_slint(&path);
                        } else if path.extension().and_then(|e| e.to_str()) == Some("slint") {
                            panic!("Accidental platform fork: found .slint file in platform dir: {:?}", path);
                        }
                    }
                }
            }
            check_no_slint(&platform_dir);
        }

        // Verify only ui/desktop contains .slint files (root and components)
        let ui_dir = root.join("ui");
        let mut slint_files = Vec::new();
        fn collect_slint(dir: &Path, list: &mut Vec<String>) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        collect_slint(&path, list);
                    } else if path.extension().and_then(|e| e.to_str()) == Some("slint") {
                        list.push(path.file_name().unwrap().to_string_lossy().to_string());
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
            slint_files,
            expected,
            "All .slint files must reside under ui/desktop/"
        );
    }
}

#[test]
fn test_production_build_path_does_not_require_spike_feature() {
    // When the slint-ui-spike feature is not active, standard Echolet core still functions
    // without any dependency on Slint.
    let runtime_state = echolet::ui::control_surface::RuntimeState::Ready;
    assert_eq!(runtime_state.status_label(), "Status: Ready");
}
