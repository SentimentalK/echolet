use crate::actions::AppAction;
use crate::asr::{OnlineRecognizer, OnlineStream};
use crate::audio::{AudioChunk, AudioInput, AudioSource, AudioStarter};
use crate::beep::{beep_start, beep_stop};
use crate::config::EcholetConfig;
use crate::diff::PartialSession;
use crate::history::HistoryManager;
use crate::models::{download_and_install_model, ModelManager};
use crate::paths;
use crate::platform::PlatformRuntime;
use crate::state::AppState;
use chrono::{DateTime, Local};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

fn default_audio_starter() -> AudioStarter {
    Box::new(|tx| AudioInput::start(tx).map(|ai| Box::new(ai) as Box<dyn AudioSource>))
}

pub struct App {
    pub state: AppState,
    pub config: EcholetConfig,
    pub model_manager: ModelManager,
    pub history_manager: HistoryManager,
    stream: Option<OnlineStream>,
    _recognizer: Option<Arc<OnlineRecognizer>>,
    session: PartialSession,
    _audio_source: Option<Box<dyn AudioSource>>,
    audio_starter: AudioStarter,
    audio_tx: Sender<AudioChunk>,
    audio_rx: Receiver<AudioChunk>,
    action_rx: Receiver<AppAction>,
    action_tx: Option<Sender<AppAction>>,
    platform: PlatformRuntime,
    last_logged_text: String,
    current_utterance_start: Option<DateTime<Local>>,
    idle_unload_deadline: Option<std::time::Instant>,
    idle_unload_model_id: Option<String>,
}

impl App {
    pub fn new(
        platform: PlatformRuntime,
        action_rx: Receiver<AppAction>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
        let starter = default_audio_starter();
        println!("[Audio] Microphone deferred until Listening starts.");
        Self::new_internal(
            platform, None, action_rx, audio_rx, audio_tx, starter, None, None,
        )
    }

    pub fn new_with_config(
        platform: PlatformRuntime,
        action_rx: Receiver<AppAction>,
        config: EcholetConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
        let starter = default_audio_starter();
        println!("[Audio] Microphone deferred until Listening starts.");
        Self::new_internal(
            platform,
            None,
            action_rx,
            audio_rx,
            audio_tx,
            starter,
            None,
            Some(config),
        )
    }

    pub fn new_with_tx(
        platform: PlatformRuntime,
        action_tx: Sender<AppAction>,
        action_rx: Receiver<AppAction>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
        let starter = default_audio_starter();
        println!("[Audio] Microphone deferred until Listening starts.");
        Self::new_internal(
            platform,
            Some(action_tx),
            action_rx,
            audio_rx,
            audio_tx,
            starter,
            None,
            None,
        )
    }

    pub fn new_with_audio(
        platform: PlatformRuntime,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_input: Option<AudioInput>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (audio_tx, _) = unbounded::<AudioChunk>();
        let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));
        let initial_source: Option<Box<dyn AudioSource>> =
            audio_input.map(|ai| Box::new(ai) as Box<dyn AudioSource>);
        Self::new_internal(
            platform,
            None,
            action_rx,
            audio_rx,
            audio_tx,
            starter,
            initial_source,
            None,
        )
    }

    pub fn new_with_starter(
        platform: PlatformRuntime,
        action_tx: Option<Sender<AppAction>>,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_tx: Sender<AudioChunk>,
        starter: AudioStarter,
        initial_source: Option<Box<dyn AudioSource>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_internal(
            platform,
            action_tx,
            action_rx,
            audio_rx,
            audio_tx,
            starter,
            initial_source,
            None,
        )
    }

    pub fn new_with_starter_and_config(
        platform: PlatformRuntime,
        action_tx: Option<Sender<AppAction>>,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_tx: Sender<AudioChunk>,
        starter: AudioStarter,
        initial_source: Option<Box<dyn AudioSource>>,
        config: Option<EcholetConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_internal(
            platform,
            action_tx,
            action_rx,
            audio_rx,
            audio_tx,
            starter,
            initial_source,
            config,
        )
    }

    pub fn new_internal(
        platform: PlatformRuntime,
        action_tx: Option<Sender<AppAction>>,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_tx: Sender<AudioChunk>,
        audio_starter: AudioStarter,
        audio_source: Option<Box<dyn AudioSource>>,
        custom_config: Option<EcholetConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = custom_config.unwrap_or_else(EcholetConfig::load);
        let history_dir = paths::history_dir();
        let history_manager = HistoryManager::new(config.history_enabled, history_dir);

        crate::log::log("INFO", "initializing ModelManager");
        let model_manager =
            ModelManager::new().map_err(|e| format!("Failed to initialize ModelManager: {}", e))?;

        // Initial UI projection
        platform.handle.set_listening(false);
        platform.handle.update_history_state(config.history_enabled);
        notify_platform_models(&platform, &model_manager);

        let mut app = Self {
            state: AppState::new(),
            config,
            model_manager,
            history_manager,
            stream: None,
            _recognizer: None,
            session: PartialSession::new(),
            _audio_source: audio_source,
            audio_starter,
            audio_tx,
            audio_rx,
            action_rx,
            action_tx,
            platform,
            last_logged_text: String::new(),
            current_utterance_start: None,
            idle_unload_deadline: None,
            idle_unload_model_id: None,
        };

        // If preload is requested, load active model during startup and begin idle deadline
        if app.config.preload_model_on_startup {
            app.ensure_model_loaded()?;
            app.schedule_idle_unload();
        }

        Ok(app)
    }

    pub fn is_model_loaded(&self) -> bool {
        self._recognizer.is_some() && self.stream.is_some()
    }

    /// Ensures the active ASR model and stream are loaded and ready for recognition.
    /// Returns Ok(()) immediately if recognizer and stream are already valid.
    /// Otherwise, loads the active model from ModelManager, creates recognizer and stream,
    /// and only publishes them after both succeed. On failure, leaves App in a clean unloaded state.
    pub fn ensure_model_loaded(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_model_loaded() {
            return Ok(());
        }

        let t_total_start = std::time::Instant::now();

        // Clean up any partial/inconsistent state before attempting load
        self.unload_model();

        let active_model = self
            .model_manager
            .get_active_model()
            .map_err(|e| format!("Failed to get active model: {}", e))?;

        println!(
            "[ASR] Active model: {} ({}) at {:?}",
            active_model.manifest.display_name, active_model.id, active_model.dir
        );

        crate::log::log("INFO", &format!("loading ASR model: {}", active_model.id));
        let t_recog_start = std::time::Instant::now();
        let recognizer = Arc::new(OnlineRecognizer::from_manifest(
            &active_model.dir,
            &active_model.manifest,
        )?);
        let recog_duration = t_recog_start.elapsed();

        let t_stream_start = std::time::Instant::now();
        let stream = recognizer.create_stream()?;
        let stream_duration = t_stream_start.elapsed();
        let total_load_duration = t_total_start.elapsed();

        crate::log::log(
            "INFO",
            &format!(
                "ASR model loaded: id='{}', total={:.2}ms, recognizer={:.2}ms, stream={:.2}ms",
                active_model.id,
                total_load_duration.as_secs_f64() * 1000.0,
                recog_duration.as_secs_f64() * 1000.0,
                stream_duration.as_secs_f64() * 1000.0,
            ),
        );
        println!(
            "[ASR] Recognizer initialized successfully in {:.2}ms (recognizer: {:.2}ms, stream: {:.2}ms).",
            total_load_duration.as_secs_f64() * 1000.0,
            recog_duration.as_secs_f64() * 1000.0,
            stream_duration.as_secs_f64() * 1000.0,
        );

        // Publish only after both recognizer and stream creation succeed
        self._recognizer = Some(recognizer);
        self.stream = Some(stream);
        Ok(())
    }

    /// Safely unloads the active model and stream runtime.
    /// Only legal when not Listening.
    /// Finalizes and clears partial ASR session state, explicitly drops OnlineStream before
    /// the App's recognizer Arc, and is idempotent.
    pub fn unload_model(&mut self) -> bool {
        if self.state.listening {
            eprintln!("[ASR] unload_model requested while Listening; ignoring until Standby.");
            return false;
        }

        self.cancel_idle_unload();

        self.finalize_current_segment();
        self.session = PartialSession::new();

        let had_runtime = self.stream.is_some() || self._recognizer.is_some();
        let t_unload = std::time::Instant::now();

        // Explicitly drop stream BEFORE recognizer Arc to maintain strict native lifetime ordering
        drop(self.stream.take());
        drop(self._recognizer.take());

        let unload_duration = t_unload.elapsed();

        if had_runtime {
            crate::log::log(
                "INFO",
                &format!(
                    "ASR model unloaded: duration={:.2}ms",
                    unload_duration.as_secs_f64() * 1000.0
                ),
            );
            println!(
                "[ASR] Model unloaded successfully in {:.2}ms.",
                unload_duration.as_secs_f64() * 1000.0
            );
        }
        true
    }

    pub fn is_audio_active(&self) -> bool {
        self._audio_source.is_some()
    }

    pub fn notify_models(&self) {
        notify_platform_models(&self.platform, &self.model_manager);
    }

    pub fn cancel_idle_unload(&mut self) {
        self.idle_unload_deadline = None;
        self.idle_unload_model_id = None;
    }

    pub fn schedule_idle_unload(&mut self) {
        if self.state.listening || !self.is_model_loaded() {
            self.cancel_idle_unload();
            return;
        }

        match self.config.model_idle_unload_minutes {
            Some(0) => {
                self.idle_unload_deadline = Some(std::time::Instant::now());
                self.idle_unload_model_id = Some(self.model_manager.active_model_id.clone());
            }
            Some(minutes) => {
                let duration = Duration::from_secs(minutes as u64 * 60);
                self.idle_unload_deadline = Some(std::time::Instant::now() + duration);
                self.idle_unload_model_id = Some(self.model_manager.active_model_id.clone());
            }
            None => {
                self.cancel_idle_unload();
            }
        }
    }

    pub fn check_idle_unload(&mut self) {
        if self.state.listening || !self.is_model_loaded() {
            return;
        }

        if let Some(deadline) = self.idle_unload_deadline {
            if let Some(ref scheduled_model) = self.idle_unload_model_id {
                if scheduled_model != &self.model_manager.active_model_id {
                    // Stale deadline from previous model
                    self.cancel_idle_unload();
                    return;
                }
            }

            if std::time::Instant::now() >= deadline {
                crate::log::log(
                    "INFO",
                    "idle unload deadline reached; unloading active model",
                );
                self.unload_model();
            }
        }
    }

    pub fn idle_unload_deadline(&self) -> Option<std::time::Instant> {
        self.idle_unload_deadline
    }

    pub fn idle_unload_model_id(&self) -> Option<&str> {
        self.idle_unload_model_id.as_deref()
    }

    pub fn set_idle_unload_deadline(&mut self, deadline: Option<std::time::Instant>) {
        self.idle_unload_deadline = deadline;
    }

    pub fn expire_idle_unload_deadline(&mut self) {
        self.idle_unload_deadline = Some(std::time::Instant::now() - Duration::from_secs(1));
        if self.idle_unload_model_id.is_none() {
            self.idle_unload_model_id = Some(self.model_manager.active_model_id.clone());
        }
    }

    pub fn start_listening(&mut self) {
        if self.state.listening {
            return;
        }

        let t_start = std::time::Instant::now();

        // Cancel any pending idle unload before/while ensuring the runtime
        self.cancel_idle_unload();

        // 1. Ensure model runtime is loaded before opening microphone / entering Listening
        let was_loaded = self.is_model_loaded();
        if let Err(err) = self.ensure_model_loaded() {
            eprintln!(
                "[ASR] Failed to ensure model is loaded before listening: {}. Remaining in Standby.",
                err
            );
            return;
        }
        let t_model_ready = t_start.elapsed();

        // 2. Open microphone on demand
        let t_mic_start = std::time::Instant::now();
        match (self.audio_starter)(self.audio_tx.clone()) {
            Ok(source) => {
                self._audio_source = Some(source);
                println!("[Audio] Microphone capture started.");
            }
            Err(err) => {
                eprintln!(
                    "[Audio] Failed to open microphone: {}. Remaining in Standby.",
                    err
                );
                self.schedule_idle_unload();
                return;
            }
        }
        let mic_duration = t_mic_start.elapsed();

        // 3. Transition state
        self.finalize_current_segment();
        self.state.listening = true;
        beep_start();
        self.platform.handle.set_listening(true);
        let total_transition_duration = t_start.elapsed();

        crate::log::log(
            "INFO",
            &format!(
                "start_listening ready: total={:.2}ms, model_ready={:.2}ms (warm={}), mic_open={:.2}ms",
                total_transition_duration.as_secs_f64() * 1000.0,
                t_model_ready.as_secs_f64() * 1000.0,
                was_loaded,
                mic_duration.as_secs_f64() * 1000.0,
            ),
        );
        println!(
            "[ASR] Listening ready in {:.2}ms (model: {:.2}ms [{}], mic: {:.2}ms).",
            total_transition_duration.as_secs_f64() * 1000.0,
            t_model_ready.as_secs_f64() * 1000.0,
            if was_loaded { "warm" } else { "loaded" },
            mic_duration.as_secs_f64() * 1000.0,
        );
        println!("\n[Action] >>> Listening STARTED (Speaking...) <<<");
    }

    pub fn stop_listening(&mut self) {
        if !self.state.listening {
            return;
        }

        // 1. Flush any pending finalized text to history before resetting ASR
        if !self.last_logged_text.is_empty() {
            let end_time = Local::now();
            let start_time = self.current_utterance_start.take().unwrap_or(end_time);
            self.history_manager.on_utterance(
                start_time,
                end_time,
                &self.last_logged_text,
                &self.model_manager.active_model_id,
            );
        }
        self.history_manager.flush();

        // 2. Finalize current segment
        self.finalize_current_segment();

        // 3. Drop audio capture stream (releases cpal::Stream & hardware device)
        self._audio_source = None;

        // 4. Drain residual audio chunks from channel
        while self.audio_rx.try_recv().is_ok() {}
        println!("[Audio] Microphone capture stopped and released.");

        // 5. Transition state
        self.state.listening = false;
        beep_stop();
        self.platform.handle.set_listening(false);
        println!("\n[Action] >>> Listening STOPPED (Standby) <<<\n");

        // 6. Schedule unload according to policy
        self.schedule_idle_unload();
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
        if let Some(ref stream) = self.stream {
            stream.reset();
        }
        self.last_logged_text.clear();
        self.current_utterance_start = None;
    }

    /// Transactionally switches active model to `model_id`.
    /// Preserves existing active model intact if candidate model initialization fails.
    pub fn select_model(&mut self, model_id: &str) -> bool {
        if self.state.listening {
            println!("[Model] Model switch requested while Listening; ignoring until Standby.");
            return false;
        }

        if self.model_manager.active_model_id == model_id {
            if self.is_model_loaded() {
                self.schedule_idle_unload();
                return true;
            }
            if self.ensure_model_loaded().is_ok() {
                self.schedule_idle_unload();
                return true;
            }
            return false;
        }

        // If installed, perform transactional switch
        if let Some(candidate) = self.model_manager.get_model(model_id).cloned() {
            println!(
                "[Model] Switching to installed model '{}' ({:?})...",
                candidate.id, candidate.dir
            );

            let new_rec = match OnlineRecognizer::from_manifest(&candidate.dir, &candidate.manifest)
            {
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

            // Transactional swap: explicitly drop previous stream before previous recognizer Arc
            drop(self.stream.take());
            drop(self._recognizer.take());

            self._recognizer = Some(new_rec);
            self.stream = Some(new_stream);
            self.session.finalize();
            self.session = PartialSession::new();
            self.last_logged_text.clear();
            self.current_utterance_start = None;

            let _ = self.model_manager.set_active_model(model_id);
            self.config.selected_model = model_id.to_string();
            let _ = self.config.save();
            self.notify_models();

            self.schedule_idle_unload();

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
                println!(
                    "[Model] Background download thread started for '{}'...",
                    dl_model_id
                );
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
                self.history_manager.flush();
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
                    println!(
                        "[Model] Download finished for '{}'. Updating installed models...",
                        model_id
                    );
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
            AppAction::ToggleHistory => {
                let new_enabled = !self.history_manager.enabled;
                self.history_manager.set_enabled(new_enabled);
                self.config.history_enabled = new_enabled;
                let _ = self.config.save();
                self.platform.handle.update_history_state(new_enabled);
                println!(
                    "[History] Local History toggled: {}",
                    if new_enabled { "ON" } else { "OFF" }
                );
            }
            AppAction::OpenHistoryFolder => {
                self.platform
                    .handle
                    .open_history_folder(&self.history_manager.history_dir);
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
                if let Some(ref stream) = self.stream {
                    stream.accept_waveform(chunk.sample_rate as i32, &chunk.samples);
                    got_audio = true;
                } else {
                    eprintln!("[ASR] Invariant violation: Listening is true but stream is absent.");
                }
            }
        }

        // 3. Decode ASR and inject diffs if listening
        if self.state.listening && got_audio {
            if let Some(ref stream) = self.stream {
                stream.decode_all_ready();

                let current_text = stream.get_result();
                let is_endpoint = stream.is_endpoint();

                // Track utterance start timestamp when first non-empty text appears
                if !current_text.is_empty() && self.current_utterance_start.is_none() {
                    self.current_utterance_start = Some(Local::now());
                }

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
                        let end_time = Local::now();
                        let start_time = self.current_utterance_start.take().unwrap_or(end_time);
                        self.history_manager.on_utterance(
                            start_time,
                            end_time,
                            &self.last_logged_text,
                            &self.model_manager.active_model_id,
                        );
                    }
                    self.finalize_current_segment();
                }
            } else {
                eprintln!("[ASR] Invariant violation: Listening is true but stream is absent during decode.");
            }
        }

        // 4. Check idle unload policy
        self.check_idle_unload();
    }

    pub fn run(&mut self) {
        while self.state.running {
            self.tick();
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.history_manager.flush();
        // Explicitly drop stream BEFORE recognizer Arc to maintain strict native lifetime ordering
        drop(self.stream.take());
        drop(self._recognizer.take());
    }
}

fn notify_platform_models(platform: &PlatformRuntime, manager: &ModelManager) {
    let installed_ids: Vec<String> = manager.installed.keys().cloned().collect();
    let downloading_ids: Vec<String> = manager.downloading.iter().cloned().collect();
    platform
        .handle
        .update_models(&manager.active_model_id, &installed_ids, &downloading_ids);
}
