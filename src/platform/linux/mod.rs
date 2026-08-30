pub mod control;
pub mod hotkey;
pub mod injector;
pub mod tray;

use crate::actions::AppAction;
use crate::platform::PlatformRuntime;
use crossbeam_channel::Sender;

pub fn handle_subcommand(args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    if args.len() > 1 && args[1] == "toggle" {
        if let Err(e) = control::send_toggle_signal() {
            eprintln!("[Error] {}", e);
            std::process::exit(1);
        }
        return Ok(true);
    }
    Ok(false)
}

pub fn init(action_tx: Sender<AppAction>) -> Result<PlatformRuntime, Box<dyn std::error::Error>> {
    // 1. Register GNOME global shortcut F10
    hotkey::register_gnome_shortcut();

    // 2. Start Unix socket control listener
    let control_handle = control::start_control_listener(action_tx.clone())?;

    // 3. Initialize Linux uinput virtual keyboard & clipboard injector
    let injector = injector::LinuxInjector::new();

    // 4. Spawn system tray icon & menu
    let tray_handle = tray::spawn_linux_tray(action_tx);

    Ok(PlatformRuntime {
        injector: Box::new(injector),
        handle: Box::new(tray_handle),
        _resources: Box::new(control_handle),
    })
}
