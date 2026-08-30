use crate::actions::AppAction;
use crate::asr::{OnlineRecognizer, OnlineStream};
use crate::audio::{AudioChunk, AudioInput};
use crate::beep::{beep_start, beep_stop};
use crate::diff::PartialSession;
use crate::paths;
use crate::platform::PlatformRuntime;
use crate::state::AppState;
use crossbeam_channel::{unbounded, Receiver};
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    pub state: AppState,
    _recognizer: Arc<OnlineRecognizer>,
    stream: OnlineStream,
    session: PartialSession,
    _audio_input: Option<AudioInput>,
    audio_rx: Receiver<AudioChunk>,
    action_rx: Receiver<AppAction>,
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

    pub fn new_with_audio(
        platform: PlatformRuntime,
        action_rx: Receiver<AppAction>,
        audio_rx: Receiver<AudioChunk>,
        audio_input: Option<AudioInput>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let model_dir = paths::default_model_dir();
        paths::validate_model_bundle(&model_dir)?;
        println!("[ASR] Model bundle validated: {:?}", model_dir);

        let recognizer = Arc::new(OnlineRecognizer::new(&model_dir)?);
        let stream = recognizer.create_stream()?;
        println!("[ASR] Recognizer initialized successfully.");

        // Explicit initial state projection
        platform.handle.set_listening(false);

        Ok(Self {
            state: AppState::new(),
            _recognizer: recognizer,
            stream,
            session: PartialSession::new(),
            _audio_input: audio_input,
            audio_rx,
            action_rx,
            platform,
            last_logged_text: String::new(),
        })
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
