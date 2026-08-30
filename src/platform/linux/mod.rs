pub mod control;
pub mod hotkey;
pub mod injector;
pub mod setup;
pub mod tray;

use crate::actions::AppAction;
use crate::platform::PlatformRuntime;
use crossbeam_channel::Sender;

pub fn handle_subcommand(args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    if args.len() > 1 {
        match args[1].as_str() {
            "toggle" => {
                if let Err(e) = control::send_toggle_signal() {
                    eprintln!("[Error] {}", e);
                    std::process::exit(1);
                }
                return Ok(true);
            }
            "stop" | "quit" => {
                if let Err(e) = control::send_stop_signal() {
                    eprintln!("[Error] {}", e);
                    std::process::exit(1);
                }
                return Ok(true);
            }
            "status" => {
                if let Err(e) = control::send_status_signal() {
                    eprintln!("[Status] {}", e);
                    std::process::exit(1);
                }
                return Ok(true);
            }
            "setup-uinput" => {
                if let Err(e) = setup::handle_setup_uinput_subcommand() {
                    eprintln!("[Setup Error] {}", e);
                    std::process::exit(1);
                }
                return Ok(true);
            }
            _ => {}
        }
    }
    Ok(false)
}

pub fn init(action_tx: Sender<AppAction>) -> Result<Option<PlatformRuntime>, Box<dyn std::error::Error>> {
    // 1. Start Unix socket control listener (Single Instance check)
    let control_handle = match control::start_control_listener(action_tx.clone())? {
        Some(handle) => handle,
        None => return Ok(None), // Duplicate instance, exit cleanly
    };

    // 2. Register GNOME global shortcut F10
    hotkey::register_gnome_shortcut();

    // 3. Initialize Linux uinput virtual keyboard & clipboard injector
    let injector = injector::LinuxInjector::new();

    // 4. Spawn system tray icon & menu
    let tray_handle = tray::spawn_linux_tray(action_tx);

    Ok(Some(PlatformRuntime {
        injector: Box::new(injector),
        handle: Box::new(tray_handle),
        _resources: Box::new(control_handle),
    }))
}
