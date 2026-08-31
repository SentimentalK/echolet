pub mod control;
pub mod hotkey;
pub mod injector;
pub mod ui;

use crate::actions::AppAction;
use crate::app::App;
use crate::paths;
use crate::platform::PlatformRuntime;
use crossbeam_channel::unbounded;
use crossbeam_channel::Sender;
use std::thread;

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
    let (cmd_tx, _cmd_rx) = unbounded::<ui::MacUiCommand>();
    let handle = Box::new(ui::MacPlatformHandle::new(cmd_tx.clone()));
    let injector = Box::new(injector::MacInjector::new(cmd_tx));

    let control_handle = match control::start_control_listener(action_tx)? {
        Some(h) => h,
        None => return Ok(None),
    };

    Ok(Some(PlatformRuntime {
        injector,
        handle,
        _resources: Box::new(control_handle),
    }))
}

pub fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    let (action_tx, action_rx) = unbounded::<AppAction>();

    // 1. Single-instance check & Unix IPC socket
    let control_handle = match control::start_control_listener(action_tx.clone())? {
        Some(handle) => handle,
        None => {
            println!("[Echolet] Existing instance active. Exiting cleanly.");
            return Ok(());
        }
    };

    // 2. Set up OS signal handler (Ctrl+C -> AppAction::Quit)
    {
        let tx = action_tx.clone();
        let _ = ctrlc::set_handler(move || {
            let _ = tx.send(AppAction::Quit);
        });
    }

    // 3. Platform Command Channel (Core -> Main Thread UI)
    let (cmd_tx, cmd_rx) = unbounded::<ui::MacUiCommand>();
    let handle = Box::new(ui::MacPlatformHandle::new(cmd_tx.clone()));
    let injector = Box::new(injector::MacInjector::new(cmd_tx));

    let platform_runtime = PlatformRuntime {
        injector,
        handle,
        _resources: Box::new(control_handle),
    };

    println!("============================================================");
    println!(" Echolet macOS Desktop - Real-time Streaming Voice Input");
    println!(" Resource Root: {:?}", paths::resource_root());
    println!("============================================================");
    println!(">>> Ready! Focus any text field (Chrome, Safari, VS Code, Terminal) <<<");
    println!(">>> Press [F10] or click Menu Bar icon to speak. <<<\n");

    // 4. Spawn Core Worker Thread
    let core_tx = action_tx.clone();
    let core_thread = thread::Builder::new()
        .name("echolet-core".into())
        .spawn(move || {
            match App::new_with_tx(platform_runtime, core_tx, action_rx) {
                Ok(mut app) => app.run(),
                Err(e) => eprintln!("[Core Error] Failed to initialize App: {}", e),
            }
        })?;

    // 5. Run AppKit Event Loop on Main Thread
    let mac_ui = ui::MacUi::new(action_tx, cmd_rx)?;
    mac_ui.run();

    let _ = core_thread.join();
    println!("[System] Exited cleanly.");
    Ok(())
}
