pub mod control;
pub mod hotkey;
pub mod icon;
pub mod injector;
pub mod singleton;
pub mod ui;

use crate::actions::AppAction;
use crate::platform::PlatformRuntime;
use crossbeam_channel::unbounded;
use crossbeam_channel::Sender;
use std::sync::atomic::AtomicIsize;
use std::sync::Arc;

pub fn handle_subcommand(args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    if args.len() > 1 {
        match args[1].as_str() {
            "toggle" => {
                match control::send_command("toggle") {
                    Ok(resp) => println!("[Echolet] {}", resp),
                    Err(e) => {
                        eprintln!("[Error] {}", e);
                        std::process::exit(1);
                    }
                }
                return Ok(true);
            }
            "stop" | "quit" => {
                match control::send_command("stop") {
                    Ok(resp) => println!("[Echolet] {}", resp),
                    Err(e) => {
                        eprintln!("[Error] {}", e);
                        std::process::exit(1);
                    }
                }
                return Ok(true);
            }
            "status" => {
                match control::send_command("status") {
                    Ok(resp) => println!("[Echolet] {}", resp),
                    Err(e) => {
                        eprintln!("[Status] {}", e);
                        std::process::exit(1);
                    }
                }
                return Ok(true);
            }
            _ => {}
        }
    }
    Ok(false)
}

pub fn init(
    action_tx: Sender<AppAction>,
) -> Result<Option<PlatformRuntime>, Box<dyn std::error::Error>> {
    // 1. Single instance check via Named Mutex
    let singleton_guard = match singleton::acquire_single_instance() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            println!("[Echolet] Another instance is already running. Exiting cleanly.");
            return Ok(None);
        }
        Err(e) => {
            eprintln!("[Warning] Failed to check single instance mutex: {}", e);
            return Err(e.into());
        }
    };

    // 2. Start Named Pipe control listener
    let control_handle = control::start_control_listener(action_tx.clone())?;

    // 3. Setup UI Thread and Command Channel
    let (cmd_tx, cmd_rx) = unbounded::<ui::WindowsUiCommand>();
    let hwnd_atomic = Arc::new(AtomicIsize::new(0));

    ui::spawn_ui_thread(action_tx, cmd_rx, hwnd_atomic.clone())?;

    let handle = Box::new(ui::WindowsPlatformHandle::new(cmd_tx, hwnd_atomic));
    let injector = Box::new(injector::WindowsInjector::new());

    Ok(Some(PlatformRuntime {
        injector,
        handle,
        _resources: Box::new((singleton_guard, control_handle)),
    }))
}
