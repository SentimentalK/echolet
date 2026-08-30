use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use crossbeam_channel::unbounded;
use echolet::asr::OnlineRecognizer;
use echolet::audio::{AudioChunk, AudioInput};
use echolet::beep::{beep_start, beep_stop};
use echolet::diff::PartialSession;
use echolet::injector::WaylandInjector;

const MODEL_DIR: &str =
    "/home/sentimentalk/sherpa-onnx/sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16";

fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("voice_input_toggle.sock")
    } else {
        PathBuf::from("/tmp/voice_input_toggle.sock")
    }
}

fn send_toggle_signal() -> Result<(), String> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("Failed to connect to daemon socket at {:?}: {}", socket_path, e))?;
    stream
        .write_all(b"TOGGLE\n")
        .map_err(|e| format!("Failed to write to socket: {}", e))?;
    println!("[Client] Sent toggle command to running instance.");
    Ok(())
}

fn register_gnome_shortcut() {
    if let Ok(exe_path) = env::current_exe() {
        let exe_str = exe_path.to_string_lossy();
        let cmd = format!("{} toggle", exe_str);

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys",
                "custom-keybindings",
                "['/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/']",
            ])
            .output();

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/",
                "name",
                "Voice Input Toggle",
            ])
            .output();

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/",
                "command",
                &cmd,
            ])
            .output();

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/",
                "binding",
                "F10",
            ])
            .output();

        println!("[Hotkey] Global shortcut F10 registered via GNOME: `{}`", cmd);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && args[1] == "toggle" {
        if let Err(e) = send_toggle_signal() {
            eprintln!("[Error] {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }

    println!("============================================================");
    println!(" Real-time Streaming Voice Input (Ubuntu Wayland) ");
    println!("============================================================");

    // Register GNOME global shortcut F10
    register_gnome_shortcut();

    let socket_path = get_socket_path();
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    println!("[Control] Listening for toggle events on {:?}", socket_path);

    println!("[ASR] Loading model from: {}", MODEL_DIR);
    let recognizer = Arc::new(OnlineRecognizer::new(MODEL_DIR)?);
    let stream = recognizer.create_stream()?;
    println!("[ASR] Recognizer initialized successfully.");

    let injector = WaylandInjector::new();
    let mut session = PartialSession::new();

    let (tx, rx) = unbounded::<AudioChunk>();
    let _audio = AudioInput::start(tx)?;
    println!("[Audio] Microphone capture active (continuous stream).");

    let is_recording = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));

    {
        let r = running.clone();
        ctrlc::set_handler(move || {
            println!("\n[System] Exiting...");
            r.store(false, Ordering::SeqCst);
        })?;
    }

    println!("\n>>> Ready! Focus any text field (ChatGPT in Chrome, VS Code, Terminal) <<<");
    println!(">>> Press [F10] to start speaking, press [F10] again to stop. <<<\n");

    let mut last_logged_text = String::new();

    while running.load(Ordering::SeqCst) {
        // Check for incoming socket toggle requests
        if let Ok((mut client, _)) = listener.accept() {
            let mut buf = [0u8; 16];
            let _ = client.read(&mut buf);
            let current = is_recording.load(Ordering::SeqCst);
            let next = !current;
            is_recording.store(next, Ordering::SeqCst);

            if next {
                beep_start();
                println!("\n[Hotkey] [F10] >>> Recording STARTED (Speaking...) <<<");
                session.finalize();
                stream.reset();
                last_logged_text.clear();
            } else {
                beep_stop();
                println!("\n[Hotkey] [F10] >>> Recording STOPPED (Standby) <<<\n");
                session.finalize();
                stream.reset();
                last_logged_text.clear();
            }
        }

        let recording = is_recording.load(Ordering::SeqCst);

        // Process audio buffer
        let mut got_audio = false;
        while let Ok(chunk) = rx.try_recv() {
            if recording && !chunk.samples.is_empty() {
                stream.accept_waveform(chunk.sample_rate as i32, &chunk.samples);
                got_audio = true;
            }
        }

        if recording && got_audio {
            stream.decode_all_ready();

            let current_text = stream.get_result();
            let is_endpoint = stream.is_endpoint();

            if let Some(diff) = session.update(&current_text) {
                if !current_text.is_empty() && current_text != last_logged_text {
                    println!(
                        "[Typing] Partial: \"{}\" | Diff: (BS: {}, Suffix: \"{}\")",
                        current_text, diff.backspaces, diff.new_suffix
                    );
                    last_logged_text = current_text.clone();
                }

                // Inject into active focused window via uinput & clipboard paste
                injector.apply_diff(diff.backspaces, &diff.new_suffix);
            }

            if is_endpoint {
                if !last_logged_text.is_empty() {
                    println!("[Endpoint] Finalized utterance: \"{}\"", last_logged_text);
                }
                session.finalize();
                stream.reset();
                last_logged_text.clear();
            }
        }

        std::thread::sleep(Duration::from_millis(15));
    }

    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    Ok(())
}
