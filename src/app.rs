use crate::actions::AppAction;
use crate::asr::{OnlineRecognizer, OnlineStream};
use crate::audio::{AudioChunk, AudioInput, AudioSource, AudioStarter};
use crate::beep::{beep_start, beep_stop};
use crate::config::EcholetConfig;
use crate::diff::PartialSession;
use crate::history::HistoryManager;
use crate::models::download::DownloadStatus;
use crate::models::{
    download_and_install_model_with_progress, InstallPhase, ModelManager, ProgressThrottle,
};
use crate::paths;
use crate::platform::{PlatformRuntime, PlatformView};
use crate::state::AppState;
use crate::ui::control_surface::{
    build_control_surface_state, project_runtime_state, ControlSurfaceState, RuntimeState,
};
use chrono::{DateTime, Local};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

fn default_audio_starter() -> AudioStarter {
    Box::new(|tx| AudioInput::start(tx).map(|ai| Box::new(ai) as Box<dyn AudioSource>))
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartListeningMetrics {
    pub total_ms: f64,
    pub model_ready_ms: f64,
    pub was_model_loaded: bool,
    pub mic_open_ms: f64,
}

struct ModelSwitchPayload {
    generation: u64,
    model_id: String,
    previous_id: Option<String>,
    built: Result<(Arc<OnlineRecognizer>, OnlineStream), String>,
}

fn release_recognizer_later(stream: OnlineStream, recognizer: Arc<OnlineRecognizer>) {
    std::thread::Builder::new()
        .name("echolet-model-drop".into())
        .spawn(move || {
            drop(stream);
            drop(recognizer);
        })
        .ok();
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
    /// True while the active recognizer is being created, so the platform UI
    /// can publish LOADING before the expensive work begins.
    model_loading: bool,
    /// Bumped on every background model switch. A finished load is applied
    /// only when it still matches, so a newer click cannot be overwritten.
    switch_generation: u64,
    switch_tx: Sender<ModelSwitchPayload>,
    switch_rx: Receiver<ModelSwitchPayload>,
    /// Last projected download status per model id.
    download_progress: HashMap<String, DownloadStatus>,
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
        crate::log::log("INFO", "initializing ModelManager");
        let model_manager =
            ModelManager::new().map_err(|e| format!("Failed to initialize ModelManager: {}", e))?;

        Self::new_with_manager_and_config(
            platform,
            action_tx,
            action_rx,
            audio_rx,
            audio_tx,
            audio_starter,
            audio_source,
            custom_config,
            model_manager,
        )
    }

    pub fn new_with_manager_and_config(
        platform: PlatformRuntime,
        action_tx: Option<Sender<AppAction>>,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_tx: Sender<AudioChunk>,
        audio_starter: AudioStarter,
        audio_source: Option<Box<dyn AudioSource>>,
        custom_config: Option<EcholetConfig>,
        model_manager: ModelManager,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = custom_config.unwrap_or_else(EcholetConfig::load);
        let history_dir = paths::history_dir();
        let history_manager = HistoryManager::new(config.history_enabled, history_dir);

        let (switch_tx, switch_rx) = unbounded();
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
            model_loading: false,
            switch_generation: 0,
            switch_tx,
            switch_rx,
            download_progress: HashMap::new(),
        };

        // Initial UI projection: neutral state derived from the manager so the
        // platform never has to guess (in particular Linux must not assume the
        // registry default is installed before the manager projects it).
        app.platform.handle.set_listening(false);
        app.platform
            .handle
            .update_history_state(app.config.history_enabled);
        app.notify_models();

        // If preload is requested, load active model during startup and begin idle deadline
        if app.config.preload_model_on_startup {
            if app.model_manager.active_model_id.is_some() {
                app.ensure_model_loaded()?;
                app.schedule_idle_unload();
            } else {
                crate::log::log(
                    "INFO",
                    "preload requested but no active model installed; remaining unloaded",
                );
                println!("[ASR] Preload requested but no model is installed. Remaining unloaded.");
            }
        }

        Ok(app)
    }

    pub fn has_active_model(&self) -> bool {
        self.model_manager.active_model_id.is_some()
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
            .map_err(|e| format!("Failed to get active model: {}", e))?
            .clone();

        println!(
            "[ASR] Active model: {} ({}) at {:?}",
            active_model.manifest.display_name, active_model.id, active_model.dir
        );

        // Publish LOADING before the expensive recognizer creation so the UI can
        // show a truthful state instead of a stale READY/UNLOADED.
        self.model_loading = true;
        self.notify_models();

        crate::log::log("INFO", &format!("loading ASR model: {}", active_model.id));
        let t_recog_start = std::time::Instant::now();
        let build_result = (|| -> Result<(Arc<OnlineRecognizer>, OnlineStream), String> {
            let recognizer = Arc::new(OnlineRecognizer::from_manifest(
                &active_model.dir,
                &active_model.manifest,
            )?);
            let stream = recognizer.create_stream()?;
            Ok((recognizer, stream))
        })();
        let recog_duration = t_recog_start.elapsed();

        self.model_loading = false;

        let (recognizer, stream) = match build_result {
            Ok(built) => built,
            Err(err) => {
                self.notify_models();
                return Err(err.into());
            }
        };

        // Apply the persisted per-model language preference BEFORE any waveform
        // is ever fed to the fresh stream.
        self.apply_language_to_stream(&active_model.id, &active_model.manifest, &stream);

        let total_load_duration = t_total_start.elapsed();
        crate::log::log(
            "INFO",
            &format!(
                "ASR model loaded: id='{}', total={:.2}ms, recognizer={:.2}ms",
                active_model.id,
                total_load_duration.as_secs_f64() * 1000.0,
                recog_duration.as_secs_f64() * 1000.0,
            ),
        );
        println!(
            "[ASR] Recognizer initialized successfully in {:.2}ms.",
            total_load_duration.as_secs_f64() * 1000.0,
        );

        // Publish only after both recognizer and stream creation succeed
        self._recognizer = Some(recognizer);
        self.stream = Some(stream);
        self.notify_models();
        Ok(())
    }

    /// Resolves the forced runtime language code for `model_id` from the
    /// persisted per-model preference, validating it against `manifest`.
    ///
    /// Returns `None` for Auto, for models without language options (e.g.
    /// X-ASR), or when a stale/invalid preference was repaired back to Auto.
    pub fn resolve_language_code(
        &mut self,
        model_id: &str,
        manifest: &crate::models::manifest::ModelManifest,
    ) -> Option<String> {
        if manifest.supported_language_options().is_empty() {
            return None;
        }
        let pref = self.config.language_preference(model_id)?;
        if pref.eq_ignore_ascii_case(EcholetConfig::LANGUAGE_AUTO) {
            return None;
        }
        match manifest.validate_language_selection(Some(pref)) {
            Ok(Some(opt)) => Some(opt.runtime_code.clone()),
            Ok(None) => None,
            Err(err) => {
                // A catalog change can invalidate a stored locale. Fall back to
                // Auto and repair the config rather than bricking model use.
                crate::log::log(
                    "WARN",
                    &format!(
                        "invalid stored language preference {:?} for model '{}': {}; resetting to Auto",
                        pref, model_id, err
                    ),
                );
                self.config.set_language_preference(model_id, None);
                let _ = self.config.save();
                None
            }
        }
    }

    /// Applies the resolved language option to a freshly created stream. Models
    /// without language options are left untouched (X-ASR keeps working with no
    /// forced language).
    fn apply_language_to_stream(
        &mut self,
        model_id: &str,
        manifest: &crate::models::manifest::ModelManifest,
        stream: &OnlineStream,
    ) {
        if manifest.supported_language_options().is_empty() {
            return;
        }
        let code = self.resolve_language_code(model_id, manifest);
        if let Err(err) = stream.set_language(code.as_deref()) {
            crate::log::log(
                "WARN",
                &format!(
                    "failed to set language option on stream for '{}': {}; falling back to Auto",
                    model_id, err
                ),
            );
            let _ = stream.set_language(None);
        }
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
        self.notify_models();
        true
    }

    pub fn is_audio_active(&self) -> bool {
        self._audio_source.is_some()
    }

    /// Single authority for the projected runtime residency/activity state.
    pub fn runtime_state(&self) -> RuntimeState {
        project_runtime_state(
            self.model_manager.active_model_id.is_some(),
            self.is_model_loaded(),
            self.model_loading,
            self.state.listening,
        )
    }

    /// Builds the platform-neutral UI projection from current app state.
    pub fn platform_view(&self) -> PlatformView {
        let installed: HashSet<String> = self.model_manager.installed.keys().cloned().collect();
        build_control_surface_state(
            &self.model_manager.registry,
            self.model_manager.active_model_id.as_deref(),
            &installed,
            &self.model_manager.downloading,
            &self.download_progress,
            &self.config,
            self.runtime_state(),
            self.history_manager.enabled,
        )
    }

    /// Canonical ControlSurfaceState accessor.
    pub fn control_surface_state(&self) -> ControlSurfaceState {
        self.platform_view()
    }

    pub fn notify_models(&self) {
        self.platform.handle.update_models(&self.platform_view());
    }

    /// Current projected download status for a model, if any.
    pub fn download_status(&self, model_id: &str) -> Option<&DownloadStatus> {
        self.download_progress.get(model_id)
    }

    pub fn cancel_idle_unload(&mut self) {
        self.idle_unload_deadline = None;
        self.idle_unload_model_id = None;
    }

    pub fn schedule_idle_unload(&mut self) {
        if self.state.listening
            || !self.is_model_loaded()
            || self.model_manager.active_model_id.is_none()
        {
            self.cancel_idle_unload();
            return;
        }

        match idle_unload_deadline_for(
            self.config.model_idle_unload_minutes,
            std::time::Instant::now(),
        ) {
            Some(deadline) => {
                self.idle_unload_deadline = Some(deadline);
                self.idle_unload_model_id = self.model_manager.active_model_id.clone();
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
                if self.model_manager.active_model_id.as_ref() != Some(scheduled_model) {
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
            self.idle_unload_model_id = self.model_manager.active_model_id.clone();
        }
    }

    pub fn start_listening(&mut self) -> Option<StartListeningMetrics> {
        if self.state.listening {
            return None;
        }

        if self.model_manager.active_model_id.is_none() {
            let msg = "No model installed. Install/select a model first.";
            crate::log::log("WARN", msg);
            eprintln!("[ASR] {}", msg);
            return None;
        }

        if self.model_loading {
            let msg = "Model is still loading. Try again in a moment.";
            crate::log::log("WARN", msg);
            eprintln!("[ASR] {}", msg);
            return None;
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
            return None;
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
                return None;
            }
        }
        let mic_duration = t_mic_start.elapsed();

        // 3. Transition state
        self.finalize_current_segment();
        self.state.listening = true;
        beep_start();
        self.platform.handle.set_listening(true);
        self.notify_models();
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

        Some(StartListeningMetrics {
            total_ms: total_transition_duration.as_secs_f64() * 1000.0,
            model_ready_ms: t_model_ready.as_secs_f64() * 1000.0,
            was_model_loaded: was_loaded,
            mic_open_ms: mic_duration.as_secs_f64() * 1000.0,
        })
    }

    pub fn stop_listening(&mut self) {
        if !self.state.listening {
            return;
        }

        // 1. Flush any pending finalized text to history before resetting ASR
        if !self.last_logged_text.is_empty() {
            let end_time = Local::now();
            let start_time = self.current_utterance_start.take().unwrap_or(end_time);
            if let Some(ref active_id) = self.model_manager.active_model_id {
                self.history_manager.on_utterance(
                    start_time,
                    end_time,
                    &self.last_logged_text,
                    active_id,
                );
            }
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
        self.notify_models();
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

    /// Starts a model switch and returns immediately.
    ///
    /// The panel updates to the new selection and a loading status before the
    /// recognizer is built. The ONNX load runs off the core thread so the
    /// panel stays responsive. A newer switch supersedes an in-flight one.
    fn begin_model_switch(&mut self, model_id: &str) -> bool {
        if self.state.listening {
            println!("[Model] Model switch requested while Listening; ignoring until Standby.");
            return false;
        }

        if self.model_manager.active_model_id.as_deref() == Some(model_id) {
            if self.model_loading {
                return true;
            }
            if self.is_model_loaded() {
                self.schedule_idle_unload();
                return true;
            }
            return self.ensure_model_loaded().is_ok();
        }

        let Some(candidate) = self.model_manager.get_model(model_id).cloned() else {
            if self.model_manager.registry.get_model(model_id).is_some() {
                println!(
                    "[Model] Model '{}' is not installed; use the Download action first.",
                    model_id
                );
            } else {
                eprintln!("[Model] Model ID '{}' not found in registry.", model_id);
            }
            return false;
        };

        self.switch_generation = self.switch_generation.wrapping_add(1);
        let generation = self.switch_generation;
        let previous_id = self.model_manager.active_model_id.clone();
        let model_id_owned = candidate.id.clone();

        if self.model_manager.set_active_model(&model_id_owned).is_err() {
            return false;
        }
        self.config.selected_model = model_id_owned.clone();
        let _ = self.config.save();
        self.model_loading = true;
        self.cancel_idle_unload();
        self.notify_models();

        crate::log::log(
            "INFO",
            &format!("switching model in background: {}", model_id_owned),
        );
        println!(
            "[Model] Switching to installed model '{}' ({:?}) in the background...",
            candidate.id, candidate.dir
        );

        let dir = candidate.dir.clone();
        let manifest = candidate.manifest.clone();
        let tx = self.switch_tx.clone();
        let load_id = model_id_owned.clone();
        let previous_for_thread = previous_id.clone();
        let spawned = std::thread::Builder::new()
            .name("echolet-model-load".into())
            .spawn(move || {
                let built = (|| {
                    let recognizer = Arc::new(OnlineRecognizer::from_manifest(&dir, &manifest)?);
                    let stream = recognizer.create_stream()?;
                    Ok((recognizer, stream))
                })();
                let _ = tx.send(ModelSwitchPayload {
                    generation,
                    model_id: load_id,
                    previous_id: previous_for_thread,
                    built,
                });
            });
        if spawned.is_err() {
            self.model_loading = false;
            self.revert_active_model(previous_id.as_deref());
            self.notify_models();
            return false;
        }
        true
    }

    fn revert_active_model(&mut self, previous_id: Option<&str>) {
        match previous_id {
            Some(prev) => {
                let _ = self.model_manager.set_active_model(prev);
                self.config.selected_model = prev.to_string();
            }
            None => {
                self.model_manager.active_model_id = None;
                self.config.selected_model.clear();
            }
        }
        let _ = self.config.save();
    }

    fn drain_model_switches(&mut self) {
        while let Ok(payload) = self.switch_rx.try_recv() {
            self.apply_model_switch(payload);
        }
    }

    fn apply_model_switch(&mut self, payload: ModelSwitchPayload) {
        if payload.generation != self.switch_generation {
            if let Ok((recognizer, stream)) = payload.built {
                release_recognizer_later(stream, recognizer);
            }
            return;
        }

        self.model_loading = false;
        match payload.built {
            Ok((recognizer, stream)) => {
                if let Some(candidate) = self.model_manager.get_model(&payload.model_id).cloned() {
                    self.apply_language_to_stream(&candidate.id, &candidate.manifest, &stream);
                }
                if let (Some(old_stream), Some(old_recognizer)) =
                    (self.stream.take(), self._recognizer.take())
                {
                    release_recognizer_later(old_stream, old_recognizer);
                }
                self._recognizer = Some(recognizer);
                self.stream = Some(stream);
                self.session.finalize();
                self.session = PartialSession::new();
                self.last_logged_text.clear();
                self.current_utterance_start = None;
                self.schedule_idle_unload();
                self.notify_models();
                crate::log::log(
                    "INFO",
                    &format!("model switch ready: {}", payload.model_id),
                );
                println!("[Model] Active model switched to {}", payload.model_id);
            }
            Err(err) => {
                eprintln!(
                    "[Model] Error: Failed to initialize candidate model '{}': {}. Retaining previous model.",
                    payload.model_id, err
                );
                crate::log::log(
                    "WARN",
                    &format!("model switch failed for '{}': {}", payload.model_id, err),
                );
                if self.model_manager.active_model_id.as_deref() == Some(payload.model_id.as_str())
                {
                    self.revert_active_model(payload.previous_id.as_deref());
                }
                self.notify_models();
            }
        }
    }

    /// Transactionally switches active model to `model_id`.
    /// Preserves existing active model intact if candidate model initialization fails.
    pub fn select_model(&mut self, model_id: &str) -> bool {
        if self.state.listening {
            println!("[Model] Model switch requested while Listening; ignoring until Standby.");
            return false;
        }

        if self.model_manager.active_model_id.as_deref() == Some(model_id) {
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

        // If installed, perform transactional switch. Selection is deliberately
        // separate from download: an uninstalled model is never auto-downloaded
        // by selecting it.
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

            // Apply the candidate model's persisted language preference before
            // publishing the stream (and thus before any future audio).
            self.apply_language_to_stream(&candidate.id, &candidate.manifest, &new_stream);

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

        if self.model_manager.registry.get_model(model_id).is_some() {
            println!(
                "[Model] Model '{}' is not installed; use the Download action first.",
                model_id
            );
        } else {
            eprintln!("[Model] Model ID '{}' not found in registry.", model_id);
        }

        false
    }

    /// Starts a background download/install for an uninstalled registry model.
    ///
    /// This is intentionally separate from [`App::select_model`]: a successful
    /// download makes the model selectable but never changes the active model.
    pub fn start_download(&mut self, model_id: &str) -> bool {
        if self.state.listening {
            println!("[Model] Download requested while Listening; ignoring until Standby.");
            return false;
        }
        if self.model_manager.installed.contains_key(model_id) {
            println!("[Model] Model '{}' is already installed.", model_id);
            return false;
        }
        if self.model_manager.downloading.contains(model_id) {
            println!("[Model] Model '{}' is already downloading.", model_id);
            return false;
        }
        let Some(entry) = self.model_manager.registry.get_model(model_id).cloned() else {
            eprintln!("[Model] Model ID '{}' not found in registry.", model_id);
            return false;
        };

        self.model_manager.downloading.insert(model_id.to_string());
        self.download_progress
            .insert(model_id.to_string(), DownloadStatus::Starting);
        self.notify_models();

        let target_dir = self.model_manager.get_user_install_dir(model_id);
        let action_tx = self.action_tx.clone();
        let dl_model_id = model_id.to_string();
        let progress_model_id = dl_model_id.clone();

        std::thread::spawn(move || {
            println!(
                "[Model] Background download thread started for '{}'...",
                dl_model_id
            );
            let mut throttle = ProgressThrottle::new();
            let progress_tx = action_tx.clone();
            let mut on_progress = move |phase: InstallPhase| {
                let status = DownloadStatus::from_phase(&phase);
                if throttle.should_emit(status.clone()) {
                    if let Some(tx) = &progress_tx {
                        let _ = tx.send(AppAction::ModelDownloadProgress {
                            model_id: progress_model_id.clone(),
                            status,
                        });
                    }
                }
            };
            let result = download_and_install_model_with_progress(
                &entry,
                &target_dir,
                Some(&mut on_progress),
            );
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
        true
    }

    /// Applies the persisted per-model language preference for `model_id`.
    ///
    /// Only legal while not Listening. Validates against the model's typed
    /// language metadata, persists the choice, and (when the model's recognizer
    /// is loaded in Standby) updates the live stream option so the next audio
    /// uses it. Returns whether the selection was applied.
    pub fn set_language(&mut self, model_id: &str, locale: Option<&str>) -> bool {
        if self.state.listening {
            println!("[Language] Selection requested while Listening; ignoring until Standby.");
            return false;
        }

        let manifest = match self.model_manager.get_model(model_id) {
            Some(m) => m.manifest.clone(),
            None => match self.model_manager.registry.get_model(model_id) {
                Some(entry) => entry.to_manifest(),
                None => {
                    eprintln!("[Language] Model ID '{}' not found.", model_id);
                    return false;
                }
            },
        };

        if manifest.supported_language_options().is_empty() {
            println!(
                "[Language] Model '{}' does not support forced-language selection.",
                model_id
            );
            return false;
        }

        let resolved = match manifest.validate_language_selection(locale) {
            Ok(opt) => opt.map(|o| o.runtime_code.clone()),
            Err(err) => {
                eprintln!("[Language] Invalid selection for '{}': {}", model_id, err);
                return false;
            }
        };

        self.config.set_language_preference(model_id, locale);
        let _ = self.config.save();

        // If this is the active model and it is resident in Standby, update the
        // live stream option without reloading the recognizer.
        if self.model_manager.active_model_id.as_deref() == Some(model_id) {
            if let Some(ref stream) = self.stream {
                if let Err(err) = stream.set_language(resolved.as_deref()) {
                    eprintln!("[Language] Failed to update stream option: {}", err);
                }
            }
        }

        self.notify_models();
        true
    }

    /// Updates the idle-unload policy and applies it consistently to the live
    /// residency state.
    ///
    /// * While Listening the policy is persisted but not enforced; it takes
    ///   effect when listening stops (via the normal `schedule_idle_unload`).
    /// * `Some(0)` (Immediate) while resident in Standby unloads promptly.
    /// * Any other value (including `None`/Never) reschedules or cancels the
    ///   active deadline.
    pub fn set_idle_unload_policy(&mut self, minutes: Option<u32>) {
        self.config.model_idle_unload_minutes = minutes;
        let _ = self.config.save();

        if self.state.listening {
            // Do not unload the model that is actively in use; the new policy is
            // applied by stop_listening -> schedule_idle_unload.
        } else if self.is_model_loaded() {
            match minutes {
                Some(0) => {
                    self.unload_model();
                }
                _ => self.schedule_idle_unload(),
            }
        } else {
            self.cancel_idle_unload();
        }

        self.notify_models();
    }

    pub fn handle_action(&mut self, action: AppAction) {
        match action {
            AppAction::ToggleListening => self.toggle_listening(),
            AppAction::StartListening => {
                self.start_listening();
            }
            AppAction::StopListening => self.stop_listening(),
            AppAction::Quit => {
                println!("\n[App] Quit action received. Exiting...");
                self.history_manager.flush();
                self.state.running = false;
                self.platform.handle.shutdown();
            }
            AppAction::DownloadModel(model_id) => {
                self.start_download(&model_id);
            }
            AppAction::SelectModel(model_id) => {
                self.begin_model_switch(&model_id);
            }
            AppAction::ModelDownloadProgress { model_id, status } => {
                // Coalesce: only re-project when the projected status actually
                // changed (the background thread already throttles raw events).
                if self.download_progress.get(&model_id) != Some(&status) {
                    self.download_progress.insert(model_id, status);
                    self.notify_models();
                }
            }
            AppAction::ModelInstalled {
                model_id,
                success,
                error,
            } => {
                self.model_manager.downloading.remove(&model_id);
                if success {
                    println!(
                        "[Model] Download finished for '{}'. Marking installed (not auto-selecting).",
                        model_id
                    );
                    self.model_manager.discover_installed();
                    self.download_progress.remove(&model_id);
                } else {
                    eprintln!(
                        "[Model] Download or verification failed for '{}': {:?}",
                        model_id, error
                    );
                    self.download_progress
                        .insert(model_id, DownloadStatus::Failed);
                }
                self.notify_models();
            }
            AppAction::SelectLanguage { model_id, locale } => {
                self.set_language(&model_id, locale.as_deref());
            }
            AppAction::SetPreloadModelOnStartup(enabled) => {
                self.config.preload_model_on_startup = enabled;
                let _ = self.config.save();
                println!(
                    "[Config] Preload on startup set to {}",
                    if enabled { "ON" } else { "OFF" }
                );
                self.notify_models();
            }
            AppAction::SetModelIdleUnloadMinutes(minutes) => {
                println!("[Config] Idle unload policy set to {:?}", minutes);
                self.set_idle_unload_policy(minutes);
            }
            AppAction::SetHistoryEnabled(enabled) => {
                if self.history_manager.enabled != enabled {
                    self.history_manager.set_enabled(enabled);
                    self.config.history_enabled = enabled;
                    let _ = self.config.save();
                    self.platform.handle.update_history_state(enabled);
                    println!(
                        "[History] Local History set: {}",
                        if enabled { "ON" } else { "OFF" }
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
                self.notify_models();
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
        self.drain_model_switches();

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
                        if let Some(ref active_id) = self.model_manager.active_model_id {
                            self.history_manager.on_utterance(
                                start_time,
                                end_time,
                                &self.last_logged_text,
                                active_id,
                            );
                        }
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

/// Pure mapping from the accepted idle-unload policy to an absolute deadline.
///
/// This is the single scheduling authority used by
/// [`App::schedule_idle_unload`], exposed so the reschedule/cancel contract can
/// be tested without a live recognizer.
pub fn idle_unload_deadline_for(
    minutes: Option<u32>,
    now: std::time::Instant,
) -> Option<std::time::Instant> {
    match minutes {
        Some(0) => Some(now),
        Some(m) => Some(now + Duration::from_secs(m as u64 * 60)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn idle_deadline_maps_all_accepted_policies() {
        let now = Instant::now();
        assert_eq!(idle_unload_deadline_for(Some(0), now), Some(now));
        assert_eq!(
            idle_unload_deadline_for(Some(1), now),
            Some(now + Duration::from_secs(60))
        );
        assert_eq!(
            idle_unload_deadline_for(Some(10), now),
            Some(now + Duration::from_secs(600))
        );
        assert_eq!(
            idle_unload_deadline_for(Some(30), now),
            Some(now + Duration::from_secs(1800))
        );
        assert_eq!(idle_unload_deadline_for(None, now), None);
    }
}
