use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::paths;
use echolet::platform;
use std::env;
use std::process::{Command, Stdio};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // 1. Handle subcommands (toggle, stop, status, setup-uinput)
    if platform::handle_subcommand(&args)? {
        return Ok(());
    }

    // 2. Determine execution mode: Default is Background (Detach), unless -f / --foreground is passed
    let is_foreground = args.iter().any(|arg| arg == "-f" || arg == "--foreground");

    if !is_foreground {
        let exe = env::current_exe()?;
        let mut child_args: Vec<String> = args
            .into_iter()
            .skip(1)
            .collect();
        child_args.push("--foreground".to_string());

        let child = Command::new(&exe)
            .args(&child_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        println!("============================================================");
        println!(" Echolet v{} started in background (PID: {}).", APP_VERSION, child.id());
        println!(" - Press [F10] or click the System Tray icon to Speak.");
        println!(" - Run `echolet stop` or click the System Tray [Quit] to exit.");
        println!(" - (Tip: Run `echolet -f` to run in foreground with debug logs)");
        println!("============================================================");
        return Ok(());
    }

    // 3. Foreground Mode: Create unified Action Channel
    let (action_tx, action_rx) = unbounded::<AppAction>();

    // 4. Set up OS signal handler (Ctrl+C -> AppAction::Quit)
    {
        let tx = action_tx.clone();
        ctrlc::set_handler(move || {
            let _ = tx.send(AppAction::Quit);
        })?;
    }

    // 5. Initialize Platform Layer (registers hotkeys, IPC socket, single instance check, tray, virtual keyboard)
    let platform_runtime = match platform::init(action_tx.clone())? {
        Some(rt) => rt,
        None => return Ok(()), // Duplicate instance detected; exiting cleanly
    };

    println!("============================================================");
    println!(" Echolet v{} - Real-time Streaming Voice Input", APP_VERSION);
    println!(" Resource Root: {:?}", paths::resource_root());
    println!("============================================================");

    // 6. Initialize and run Core App Engine
    println!("\n>>> Ready! Focus any text field (ChatGPT in Chrome, VS Code, Terminal) <<<");
    println!(">>> Press [F10] or click Tray [Start Listening] to speak. <<<");
    println!(">>> Press [F10] again or click Tray [Stop Listening] to stop. <<<\n");

    let mut app = App::new_with_tx(platform_runtime, action_tx, action_rx)?;
    app.run();

    println!("[System] Exited cleanly.");
    Ok(())
}
