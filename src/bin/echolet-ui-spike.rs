//! Echolet Slint UI Spike Executable (PROJECT-041 / J11.1c).
//!
//! Opt-in spike runner to measure and evaluate Slint as a single shared desktop renderer
//! across Windows, macOS, and Linux.

use echolet::diagnostics::memory::get_current_rss;
use echolet::ui::desktop::{
    ComponentHandle, DesktopPanelViewModel, EcholetPanel, SlintControlSurfaceAdapter,
    PANEL_HEIGHT_PX, PANEL_WIDTH_PX,
};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

fn print_usage() {
    println!("Echolet Slint UI Spike (J11.1c)");
    println!();
    println!("USAGE:");
    println!("    echolet-ui-spike [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help                 Print help information");
    println!("    --dry-run                  Initialize and bind view model, then exit without showing window");
    println!("    --bench                    Run benchmark suite (measures startup, open/hide latency, and RSS)");
    println!("    --long-names               Use extreme length text fixture to verify layout bounds");
    println!("    --close-after-ms <MS>      Auto-close and exit event loop after specified milliseconds");
    println!("    --iterations <N>           Number of open/hide iterations for benchmarking (default: 5)");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    // Prefer Slint software renderer by default if SLINT_BACKEND not explicitly overridden.
    if env::var("SLINT_BACKEND").is_err() {
        env::set_var("SLINT_BACKEND", "winit-software");
    }

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return Ok(());
    }

    let is_dry_run = args.iter().any(|a| a == "--dry-run");
    let use_long_names = args.iter().any(|a| a == "--long-names");
    let is_bench = args.iter().any(|a| a == "--bench");
    let close_after_ms: Option<u64> = args
        .iter()
        .position(|a| a == "--close-after-ms")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());
    let iterations: usize = args
        .iter()
        .position(|a| a == "--iterations")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    println!("============================================================");
    println!(" Echolet Desktop UI Spike (Slint 1.18.1)");
    println!("============================================================");
    println!("Logical Panel Width : {} px", PANEL_WIDTH_PX);
    println!("Logical Panel Height: {} px", PANEL_HEIGHT_PX);
    println!("SLINT_BACKEND       : {}", env::var("SLINT_BACKEND").unwrap_or_default());

    // 1. Build deterministic representative fixture
    let state = if use_long_names {
        SlintControlSurfaceAdapter::create_spike_fixture_long_names()
    } else {
        SlintControlSurfaceAdapter::create_spike_fixture()
    };

    let view_model = DesktopPanelViewModel::from_control_surface(&state);

    // 2. Instantiate Slint component
    let panel = EcholetPanel::new()?;
    let init_duration = start_time.elapsed();
    println!("Component creation  : {:?}", init_duration);

    // 3. Apply view model to Slint component
    SlintControlSurfaceAdapter::apply_to_panel(&panel, &view_model);
    let bind_duration = start_time.elapsed();
    println!("ViewModel binding   : {:?}", bind_duration);

    if let Some(rss) = get_current_rss() {
        println!("Initial Process RSS : {:.2} MiB ({} bytes)", rss.mib(), rss.bytes());
    }

    if is_dry_run {
        println!("--> Dry-run completed successfully.");
        return Ok(());
    }

    // 4. Attach callbacks
    let panel_weak_close = panel.as_weak();
    panel.on_close_requested(move || {
        println!("[Event] Close requested (Escape). Hiding and quitting event loop.");
        if let Some(p) = panel_weak_close.upgrade() {
            let _ = p.hide();
        }
        let _ = slint::quit_event_loop();
    });

    let panel_weak_diag = panel.as_weak();
    let diag_vm = std::sync::Arc::new(std::sync::Mutex::new(view_model));
    let diag_vm_clone = Arc::clone(&diag_vm);
    panel.on_diagnostic_clicked(move || {
        if let Ok(mut vm) = diag_vm_clone.lock() {
            vm.trigger_diagnostic_toggle();
            println!(
                "[Event] Diagnostic clicked -> state: {}, count: {}",
                vm.diagnostic_state, vm.diagnostic_count
            );
            if let Some(p) = panel_weak_diag.upgrade() {
                SlintControlSurfaceAdapter::apply_to_panel(&p, &vm);
            }
        }
    });

    let diag_vm_action = Arc::clone(&diag_vm);
    panel.on_model_action_clicked(move |model_id| {
        if let Ok(vm) = diag_vm_action.lock() {
            let action = vm.resolve_surface_action(model_id.as_str());
            println!(
                "[Event] Model action clicked for '{}' -> resolved SurfaceAction: {:?}",
                model_id, action
            );
        }
    });

    // 5. Position window (deterministic test anchor)
    panel.window().set_position(slint::LogicalPosition::new(100.0, 100.0));

    // 6. Benchmark repeated open/hide cycles if requested
    if is_bench {
        println!("--- Starting repeated open/hide benchmark ({} iterations) ---", iterations);
        let mut open_latencies = Vec::new();
        let mut hide_latencies = Vec::new();

        for i in 0..iterations {
            let t_open = Instant::now();
            panel.show()?;
            let d_open = t_open.elapsed();
            open_latencies.push(d_open);

            if i == 0 {
                if let Some(rss) = get_current_rss() {
                    println!("[Bench] Visible window RSS (iteration 1): {:.2} MiB ({} bytes)", rss.mib(), rss.bytes());
                }
            }

            let t_hide = Instant::now();
            panel.hide()?;
            let d_hide = t_hide.elapsed();
            hide_latencies.push(d_hide);
        }

        if let Some(rss) = get_current_rss() {
            println!("[Bench] Hidden window RSS (after {} iterations): {:.2} MiB ({} bytes)", iterations, rss.mib(), rss.bytes());
        }

        open_latencies.sort();
        hide_latencies.sort();

        let median_open = open_latencies[open_latencies.len() / 2];
        let min_open = open_latencies.first().unwrap();
        let max_open = open_latencies.last().unwrap();

        println!(
            "[Bench Results] Open Latency  : median = {:?}, min = {:?}, max = {:?}",
            median_open, min_open, max_open
        );

        let median_hide = hide_latencies[hide_latencies.len() / 2];
        let min_hide = hide_latencies.first().unwrap();
        let max_hide = hide_latencies.last().unwrap();

        println!(
            "[Bench Results] Hide Latency  : median = {:?}, min = {:?}, max = {:?}",
            median_hide, min_hide, max_hide
        );

        if close_after_ms.is_none() {
            println!("--> Benchmark completed. Exiting.");
            return Ok(());
        }
    }

    // 7. Auto-close timer (must stay alive in main scope)
    let panel_weak_timer = panel.as_weak();
    let _timer = if let Some(ms) = close_after_ms {
        let timer = slint::Timer::default();
        let timer_fired = Arc::new(AtomicBool::new(false));
        let tf = Arc::clone(&timer_fired);
        timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(ms),
            move || {
                tf.store(true, Ordering::SeqCst);
                if let Some(rss) = get_current_rss() {
                    println!("[Timer] Stable visible RSS at {}ms: {:.2} MiB ({} bytes)", ms, rss.mib(), rss.bytes());
                }
                println!("[Timer] close-after-ms ({ms}ms) reached. Quitting event loop.");
                if let Some(p) = panel_weak_timer.upgrade() {
                    let _ = p.hide();
                }
                let _ = slint::quit_event_loop();
            },
        );
        Some(timer)
    } else {
        None
    };

    println!("--> Running Slint event loop (Press Escape to close)...");
    panel.run()?;
    println!("--> Event loop terminated successfully.");

    if let Some(rss) = get_current_rss() {
        println!("Final Process RSS   : {:.2} MiB ({} bytes)", rss.mib(), rss.bytes());
    }

    Ok(())
}
