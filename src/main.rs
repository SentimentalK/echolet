use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::paths;
use echolet::platform;
use std::env;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if platform::handle_subcommand(&args)? {
        return Ok(());
    }

    println!("============================================================");
    println!(" Echolet v{} - Real-time Streaming Voice Input", APP_VERSION);
    println!(" Resource Root: {:?}", paths::resource_root());
    println!("============================================================");

    // 1. Create unified Action Channel
    let (action_tx, action_rx) = unbounded::<AppAction>();

    // 2. Set up OS signal handler (Ctrl+C -> AppAction::Quit)
    {
        let tx = action_tx.clone();
        ctrlc::set_handler(move || {
            let _ = tx.send(AppAction::Quit);
        })?;
    }

    // 3. Initialize Platform Layer (registers hotkeys, IPC socket, tray, virtual keyboard)
    let platform_runtime = platform::init(action_tx)?;

    // 4. Initialize and run Core App Engine
    println!("\n>>> Ready! Focus any text field (ChatGPT in Chrome, VS Code, Terminal) <<<");
    println!(">>> Press [F10] or click Tray [Start Listening] to speak. <<<");
    println!(">>> Press [F10] again or click Tray [Stop Listening] to stop. <<<\n");

    let mut app = App::new(platform_runtime, action_rx)?;
    app.run();

    println!("[System] Exited cleanly.");
    Ok(())
}
