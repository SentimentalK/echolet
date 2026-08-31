use crate::actions::AppAction;
use crate::asr::{OnlineRecognizer, OnlineStream};
use crate::audio::{AudioChunk, AudioInput};
use crate::beep::{beep_start, beep_stop};
use crate::diff::PartialSession;
use crate::models::{download_and_install_model, ModelManager};
use crate::platform::PlatformRuntime;
use crate::state::AppState;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    pub state: AppState,
    pub model_manager: ModelManager,
    _recognizer: Arc<OnlineRecognizer>,
    stream: OnlineStream,
    session: PartialSession,
    _audio_input: Option<AudioInput>,
    audio_rx: Receiver<AudioChunk>,
    action_rx: Receiver<AppAction>,
    action_tx: Option<Sender<AppAction>>,
    platform: PlatformRuntime,
    last_logged_text: String,
}

impl App {
    pub fn new(
        platform: PlatformRuntime,
        action_rx: Receiver<AppAction>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
        let audio_input = AudioInput::start(audio_tx)?;
        println!("[Audio] Microphone capture active (continuous stream).");
        Self::new_with_audio(platform, action_rx, audio_rx, Some(audio_input))
    }

    pub fn new_with_tx(
        platform: PlatformRuntime,
        action_tx: Sender<AppAction>,
        action_rx: Receiver<AppAction>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
        let audio_input = AudioInput::start(audio_tx)?;
        println!("[Audio] Microphone capture active (continuous stream).");
        Self::new_internal(platform, Some(action_tx), action_rx, audio_rx, Some(audio_input))
    }

    pub fn new_with_audio(
        platform: PlatformRuntime,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_input: Option<AudioInput>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_internal(platform, None, action_rx, audio_rx, audio_input)
    }

    pub fn new_internal(
        platform: PlatformRuntime,
        action_tx: Option<Sender<AppAction>>,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_input: Option<AudioInput>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let model_manager = ModelManager::new().map_err(|e| format!("Failed to initialize ModelManager: {}", e))?;
        let active_model = model_manager.get_active_model().map_err(|e| format!("Failed to get active model: {}", e))?;

        println!(
            "[ASR] Active model: {} ({}) at {:?}",
            active_model.manifest.display_name, active_model.id, active_model.dir
        );

        let recognizer = Arc::new(OnlineRecognizer::from_manifest(&active_model.dir, &active_model.manifest)?);
        let stream = recognizer.create_stream()?;
        println!("[ASR] Recognizer initialized successfully.");

        // Initial UI projection
        platform.handle.set_listening(false);
        notify_platform_models(&platform, &model_manager);

        Ok(Self {
            state: AppState::new(),
            model_manager,
            _recognizer: recognizer,
            stream,
            session: PartialSession::new(),
            _audio_input: audio_input,
            audio_rx,
            action_rx,
            action_tx,
            platform,
            last_logged_text: String::new(),
        })
    }

    pub fn notify_models(&self) {
        notify_platform_models(&self.platform, &self.model_manager);
    }

    pub fn start_listening(&mut self) {
        if self.state.listening {
            return;
        }
        self.finalize_current_segment();
        self.state.listening = true;
        beep_start();
        self.platform.handle.set_listening(true);
        println!("\n[Action] >>> Listening STARTED (Speaking...) <<<");
    }

    pub fn stop_listening(&mut self) {
        if !self.state.listening {
            return;
        }
        self.finalize_current_segment();
        self.state.listening = false;
        beep_stop();
        self.platform.handle.set_listening(false);
        println!("\n[Action] >>> Listening STOPPED (Standby) <<<\n");
    }

    pub fn toggle_listening(&mut self) {
        if self.state.listening {
            self.stop_listening();
        } else {
            self.start_listening();
        }
    }

    /// Finalizes the current partial utterance without altering the listening state.
    pub fn finalize_current_segment(&mut self) {
        self.session.finalize();
        self.stream.reset();
        self.last_logged_text.clear();
    }

    /// Transactionally switches active model to `model_id`.
    /// Preserves existing active model intact if candidate model initialization fails.
    pub fn select_model(&mut self, model_id: &str) -> bool {
        if self.state.listening {
            println!("[Model] Model switch requested while Listening; ignoring until Standby.");
            return false;
        }

        if self.model_manager.active_model_id == model_id {
            return true;
        }

        // If installed, perform transactional switch
        if let Some(candidate) = self.model_manager.get_model(model_id).cloned() {
            println!(
                "[Model] Switching to installed model '{}' ({:?})...",
                candidate.id, candidate.dir
            );

            let new_rec = match OnlineRecognizer::from_manifest(&candidate.dir, &candidate.manifest) {
                Ok(rec) => Arc::new(rec),
                Err(err) => {
                    eprintln!(
                        "[Model] Error: Failed to initialize candidate model '{}': {}. Retaining active model.",
                        model_id, err
                    );
                    return false;
                }
            };

            let new_stream = match new_rec.create_stream() {
                Ok(st) => st,
                Err(err) => {
                    eprintln!(
                        "[Model] Error: Failed to create stream for candidate model '{}': {}. Retaining active model.",
                        model_id, err
                    );
                    return false;
                }
            };

            // Transactional swap
            self._recognizer = new_rec;
            self.stream = new_stream;
            self.session.finalize();
            self.session = PartialSession::new();
            self.last_logged_text.clear();

            let _ = self.model_manager.set_active_model(model_id);
            self.notify_models();

            println!(
                "[Model] Active model successfully switched to: {} — {}",
                candidate.manifest.display_name, candidate.manifest.version
            );
            return true;
        }

        // If not installed, trigger background download
        if let Some(entry) = self.model_manager.registry.get_model(model_id).cloned() {
            if self.model_manager.downloading.contains(model_id) {
                println!("[Model] Model '{}' is already downloading.", model_id);
                return false;
            }

            self.model_manager.downloading.insert(model_id.to_string());
            self.notify_models();

            let target_dir = self.model_manager.get_user_install_dir(model_id);
            let action_tx = self.action_tx.clone();
            let dl_model_id = model_id.to_string();

            std::thread::spawn(move || {
                println!("[Model] Background download thread started for '{}'...", dl_model_id);
                let result = download_and_install_model(&entry, &target_dir);
                let (success, error) = match result {
                    Ok(_) => (true, None),
                    Err(e) => (false, Some(e)),
                };

                if let Some(tx) = action_tx {
                    let _ = tx.send(AppAction::ModelInstalled {
                        model_id: dl_model_id,
                        success,
                        error,
                    });
                }
            });
        } else {
            eprintln!("[Model] Model ID '{}' not found in registry.", model_id);
        }

        false
    }

    pub fn handle_action(&mut self, action: AppAction) {
        match action {
            AppAction::ToggleListening => self.toggle_listening(),
            AppAction::StartListening => self.start_listening(),
            AppAction::StopListening => self.stop_listening(),
            AppAction::Quit => {
                println!("\n[App] Quit action received. Exiting...");
                self.state.running = false;
                self.platform.handle.shutdown();
            }
            AppAction::SelectModel(model_id) => {
                self.select_model(&model_id);
            }
            AppAction::ModelInstalled {
                model_id,
                success,
                error,
            } => {
                self.model_manager.downloading.remove(&model_id);
                if success {
                    println!("[Model] Download finished for '{}'. Updating installed models...", model_id);
                    self.model_manager.discover_installed();
                    self.notify_models();
                    if !self.state.listening {
                        self.select_model(&model_id);
                    }
                } else {
                    eprintln!(
                        "[Model] Download or verification failed for '{}': {:?}",
                        model_id, error
                    );
                    self.notify_models();
                }
            }
        }
    }

    /// Single tick of event draining and ASR stream decoding.
    pub fn tick(&mut self) {
        // 1. Drain pending platform actions
        while let Ok(action) = self.action_rx.try_recv() {
            self.handle_action(action);
            if !self.state.running {
                return;
            }
        }

        // 2. Process incoming audio chunks
        let mut got_audio = false;
        while let Ok(chunk) = self.audio_rx.try_recv() {
            if self.state.listening && !chunk.samples.is_empty() {
                self.stream
                    .accept_waveform(chunk.sample_rate as i32, &chunk.samples);
                got_audio = true;
            }
        }

        // 3. Decode ASR and inject diffs if listening
        if self.state.listening && got_audio {
            self.stream.decode_all_ready();

            let current_text = self.stream.get_result();
            let is_endpoint = self.stream.is_endpoint();

            if let Some(diff) = self.session.update(&current_text) {
                if !current_text.is_empty() && current_text != self.last_logged_text {
                    println!(
                        "[Typing] Partial: \"{}\" | Diff: (BS: {}, Suffix: \"{}\")",
                        current_text, diff.backspaces, diff.new_suffix
                    );
                    self.last_logged_text = current_text.clone();
                }

                // Inject into active focused window via platform text injector
                self.platform
                    .injector
                    .apply_diff(diff.backspaces, &diff.new_suffix);
            }

            // Endpoint commits current sentence segment while listening state remains active.
            if is_endpoint {
                if !self.last_logged_text.is_empty() {
                    println!(
                        "[Endpoint] Finalized sentence: \"{}\" (Listening stays active)",
                        self.last_logged_text
                    );
                }
                self.finalize_current_segment();
            }
        }
    }

    pub fn run(&mut self) {
        while self.state.running {
            self.tick();
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

fn notify_platform_models(platform: &PlatformRuntime, manager: &ModelManager) {
    let installed_ids: Vec<String> = manager.installed.keys().cloned().collect();
    let downloading_ids: Vec<String> = manager.downloading.iter().cloned().collect();
    platform.handle.update_models(
        &manager.active_model_id,
        &installed_ids,
        &downloading_ids,
    );
}
