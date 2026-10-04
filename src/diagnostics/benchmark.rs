use crate::asr::{OnlineRecognizer, OnlineStream};
use crate::diagnostics::memory::{get_current_rss, ProcessRss};
use crate::models::ModelManager;
use std::env;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WarmCycleResult {
    pub cycle: usize,
    pub unload_ms: f64,
    pub rss_after_unload: Option<ProcessRss>,
    pub recog_reload_ms: f64,
    pub stream_reload_ms: f64,
    pub total_reload_ms: f64,
    pub rss_after_reload: Option<ProcessRss>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct F10LatencyResult {
    pub unloaded_model_ready_ms: f64,
    pub unloaded_total_ms: f64,
    pub warm_model_ready_ms: f64,
    pub warm_total_ms: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkRunResult {
    pub os: String,
    pub arch: String,
    pub model_id: String,
    pub model_name: String,
    pub model_dir: String,
    pub rss_baseline: Option<ProcessRss>,
    pub fresh_load_recog_ms: f64,
    pub fresh_load_stream_ms: f64,
    pub fresh_load_total_ms: f64,
    pub rss_after_fresh_load: Option<ProcessRss>,
    pub unload_ms: f64,
    pub rss_after_unload: Option<ProcessRss>,
    pub immediate_reload_recog_ms: f64,
    pub immediate_reload_stream_ms: f64,
    pub immediate_reload_total_ms: f64,
    pub rss_after_immediate_reload: Option<ProcessRss>,
    pub warm_cycles: Vec<WarmCycleResult>,
    pub f10_latency: Option<F10LatencyResult>,
}

/// Executes a complete in-process model lifecycle measurement run.
///
/// Steps:
/// 1. Record baseline RSS before loading any model.
/// 2. Fresh model load: measures `OnlineRecognizer` construction and `OnlineStream` creation.
/// 3. Model unload: measures explicit drop time preserving J1 ordering (stream before recognizer Arc).
/// 4. Immediate reload: measures reload time when OS filesystem caches are warm.
/// 5. Warm repeated reload cycles (default: 3 cycles).
/// 6. Simulates cold and warm `start_listening` without microphone hardware.
pub fn run_in_process_benchmark(
    warm_repeat_count: usize,
) -> Result<BenchmarkRunResult, Box<dyn std::error::Error>> {
    let rss_baseline = get_current_rss();

    let model_manager =
        ModelManager::new().map_err(|e| format!("Failed to initialize ModelManager: {}", e))?;
    let active_model = model_manager
        .get_active_model()
        .map_err(|e| format!("Failed to get active model: {}", e))?;

    // --- Step 1: Fresh Process Load ---
    let t_fresh_total_start = Instant::now();
    let t_fresh_recog_start = Instant::now();
    let mut recognizer = Some(Arc::new(OnlineRecognizer::from_manifest(
        &active_model.dir,
        &active_model.manifest,
    )?));
    let fresh_load_recog_ms = t_fresh_recog_start.elapsed().as_secs_f64() * 1000.0;

    let t_fresh_stream_start = Instant::now();
    let mut stream = Some(recognizer.as_ref().unwrap().create_stream()?);
    let fresh_load_stream_ms = t_fresh_stream_start.elapsed().as_secs_f64() * 1000.0;
    let fresh_load_total_ms = t_fresh_total_start.elapsed().as_secs_f64() * 1000.0;

    let rss_after_fresh_load = get_current_rss();

    // --- Step 2: Unload (preserving strict J1 drop order: stream before recognizer) ---
    let t_unload_start = Instant::now();
    drop(stream.take());
    drop(recognizer.take());
    let unload_ms = t_unload_start.elapsed().as_secs_f64() * 1000.0;

    let rss_after_unload = get_current_rss();

    // --- Step 3: Immediate Reload (OS cache warm) ---
    let t_reload_total_start = Instant::now();
    let t_reload_recog_start = Instant::now();
    recognizer = Some(Arc::new(OnlineRecognizer::from_manifest(
        &active_model.dir,
        &active_model.manifest,
    )?));
    let immediate_reload_recog_ms = t_reload_recog_start.elapsed().as_secs_f64() * 1000.0;

    let t_reload_stream_start = Instant::now();
    stream = Some(recognizer.as_ref().unwrap().create_stream()?);
    let immediate_reload_stream_ms = t_reload_stream_start.elapsed().as_secs_f64() * 1000.0;
    let immediate_reload_total_ms = t_reload_total_start.elapsed().as_secs_f64() * 1000.0;

    let rss_after_immediate_reload = get_current_rss();

    // --- Step 4: Repeated Warm Reload Cycles ---
    let mut warm_cycles = Vec::with_capacity(warm_repeat_count);
    for cycle in 1..=warm_repeat_count {
        // Unload
        let t_cyc_unload_start = Instant::now();
        drop(stream.take());
        drop(recognizer.take());
        let cyc_unload_ms = t_cyc_unload_start.elapsed().as_secs_f64() * 1000.0;
        let cyc_rss_unload = get_current_rss();

        // Reload
        let t_cyc_reload_total_start = Instant::now();
        let t_cyc_recog_start = Instant::now();
        recognizer = Some(Arc::new(OnlineRecognizer::from_manifest(
            &active_model.dir,
            &active_model.manifest,
        )?));
        let cyc_recog_ms = t_cyc_recog_start.elapsed().as_secs_f64() * 1000.0;

        let t_cyc_stream_start = Instant::now();
        stream = Some(recognizer.as_ref().unwrap().create_stream()?);
        let cyc_stream_ms = t_cyc_stream_start.elapsed().as_secs_f64() * 1000.0;
        let cyc_total_ms = t_cyc_reload_total_start.elapsed().as_secs_f64() * 1000.0;
        let cyc_rss_reload = get_current_rss();

        warm_cycles.push(WarmCycleResult {
            cycle,
            unload_ms: cyc_unload_ms,
            rss_after_unload: cyc_rss_unload,
            recog_reload_ms: cyc_recog_ms,
            stream_reload_ms: cyc_stream_ms,
            total_reload_ms: cyc_total_ms,
            rss_after_reload: cyc_rss_reload,
        });
    }

    // Clean up before measuring App F10 latency
    drop(stream.take());
    drop(recognizer.take());

    // --- Step 5: User-relevant start_listening (F10) Latency ---
    let f10_latency = measure_f10_latency().ok();

    Ok(BenchmarkRunResult {
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        model_id: active_model.id.clone(),
        model_name: active_model.manifest.display_name.clone(),
        model_dir: active_model.dir.to_string_lossy().to_string(),
        rss_baseline,
        fresh_load_recog_ms,
        fresh_load_stream_ms,
        fresh_load_total_ms,
        rss_after_fresh_load,
        unload_ms,
        rss_after_unload,
        immediate_reload_recog_ms,
        immediate_reload_stream_ms,
        immediate_reload_total_ms,
        rss_after_immediate_reload,
        warm_cycles,
        f10_latency,
    })
}

fn measure_f10_latency() -> Result<F10LatencyResult, Box<dyn std::error::Error>> {
    use crate::actions::AppAction;
    use crate::app::App;
    use crate::config::EcholetConfig;
    use crate::platform::{PlatformHandle, PlatformRuntime, TextInjector};
    use crossbeam_channel::unbounded;
    use std::path::Path;

    struct BenchInjector;
    impl TextInjector for BenchInjector {
        fn apply_diff(&self, _backspaces: usize, _new_suffix: &str) {}
    }

    struct BenchPlatformHandle;
    impl PlatformHandle for BenchPlatformHandle {
        fn set_listening(&self, _listening: bool) {}
        fn shutdown(&self) {}
        fn update_history_state(&self, _enabled: bool) {}
        fn open_history_folder(&self, _history_dir: &Path) {}
    }

    let platform = PlatformRuntime {
        injector: Box::new(BenchInjector),
        handle: Box::new(BenchPlatformHandle),
        _resources: Box::new(()),
    };

    let (_, action_rx) = unbounded::<AppAction>();
    let (_, audio_rx) = unbounded();

    let mut config = EcholetConfig::load();
    config.preload_model_on_startup = false;
    config.history_enabled = false;

    let mut app = App::new_with_audio(platform, action_rx, audio_rx, None)?;
    app.unload_model();

    // 1. Cold start_listening (runtime unloaded)
    let t_cold = Instant::now();
    app.start_listening();
    let unloaded_total_ms = t_cold.elapsed().as_secs_f64() * 1000.0;
    app.stop_listening();

    // 2. Warm start_listening (runtime already loaded)
    let t_warm = Instant::now();
    app.start_listening();
    let warm_total_ms = t_warm.elapsed().as_secs_f64() * 1000.0;
    app.stop_listening();

    // Ensure final state is clean
    app.unload_model();

    Ok(F10LatencyResult {
        unloaded_model_ready_ms: unloaded_total_ms,
        unloaded_total_ms,
        warm_model_ready_ms: warm_total_ms,
        warm_total_ms,
    })
}

fn format_rss(rss: Option<ProcessRss>) -> String {
    match rss {
        Some(r) => format!("{:.2} MiB", r.mib()),
        None => "N/A".to_string(),
    }
}

fn format_delta(current: Option<ProcessRss>, baseline: Option<ProcessRss>) -> String {
    match (current, baseline) {
        (Some(curr), Some(base)) => {
            let diff_mib = curr.mib() - base.mib();
            if diff_mib >= 0.0 {
                format!("+{:.2} MiB", diff_mib)
            } else {
                format!("-{:.2} MiB", diff_mib.abs())
            }
        }
        _ => "-".to_string(),
    }
}

pub fn print_benchmark_table(result: &BenchmarkRunResult) {
    println!("=========================================================================================================");
    println!(" Echolet Model Lifecycle Benchmark (PROJECT-041 Stage 1 / J3)");
    println!("=========================================================================================================");
    println!(" Platform: {} ({})", result.os, result.arch);
    println!(" Model:    {} ({})", result.model_name, result.model_id);
    println!(" Location: {}", result.model_dir);
    println!("---------------------------------------------------------------------------------------------------------");
    println!(
        " {:<28} | {:>14} | {:>11} | {:>10} | {:>15} | {:>10}",
        "Phase / Step", "Recognizer (ms)", "Stream (ms)", "Total (ms)", "Current RSS", "Delta"
    );
    println!("---------------------------------------------------------------------------------------------------------");

    println!(
        " {:<28} | {:>14} | {:>11} | {:>10} | {:>15} | {:>10}",
        "Baseline (before load)",
        "-",
        "-",
        "-",
        format_rss(result.rss_baseline),
        "-"
    );

    println!(
        " {:<28} | {:>14.2} | {:>11.2} | {:>10.2} | {:>15} | {:>10}",
        "Fresh Process Load",
        result.fresh_load_recog_ms,
        result.fresh_load_stream_ms,
        result.fresh_load_total_ms,
        format_rss(result.rss_after_fresh_load),
        format_delta(result.rss_after_fresh_load, result.rss_baseline)
    );

    println!(
        " {:<28} | {:>14} | {:>11} | {:>10.2} | {:>15} | {:>10}",
        "Model Unload",
        "-",
        "-",
        result.unload_ms,
        format_rss(result.rss_after_unload),
        format_delta(result.rss_after_unload, result.rss_after_fresh_load)
    );

    println!(
        " {:<28} | {:>14.2} | {:>11.2} | {:>10.2} | {:>15} | {:>10}",
        "Immediate Reload (OS warm)",
        result.immediate_reload_recog_ms,
        result.immediate_reload_stream_ms,
        result.immediate_reload_total_ms,
        format_rss(result.rss_after_immediate_reload),
        format_delta(result.rss_after_immediate_reload, result.rss_after_unload)
    );

    for cycle in &result.warm_cycles {
        let label = format!("Warm Reload Cycle {}", cycle.cycle);
        println!(
            " {:<28} | {:>14.2} | {:>11.2} | {:>10.2} | {:>15} | {:>10}",
            label,
            cycle.recog_reload_ms,
            cycle.stream_reload_ms,
            cycle.total_reload_ms,
            format_rss(cycle.rss_after_reload),
            format_delta(cycle.rss_after_reload, cycle.rss_after_unload)
        );
    }

    if let Some(ref f10) = result.f10_latency {
        println!("---------------------------------------------------------------------------------------------------------");
        println!(" User-Facing F10 (start_listening) Latency (microsecond mock mic):");
        println!(
            "   - Cold (runtime unloaded): model_ready = {:.2}ms, total = {:.2}ms",
            f10.unloaded_model_ready_ms, f10.unloaded_total_ms
        );
        println!(
            "   - Warm (runtime loaded):   model_ready = {:.2}ms, total = {:.2}ms",
            f10.warm_model_ready_ms, f10.warm_total_ms
        );
    }

    println!("---------------------------------------------------------------------------------------------------------");
    println!(" Memory note: RSS is an OS/process resident set size observation.");
    println!(
        " Native allocator caches pages (e.g. glibc malloc arenas / mmap pools); released memory"
    );
    println!(" may remain retained by the allocator rather than immediately returning to the OS.");
    println!("=========================================================================================================\n");
}

pub fn run_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let is_child = args.iter().any(|a| a == "--child-fresh");
    let is_json = args.iter().any(|a| a == "--json");
    let is_direct = args.iter().any(|a| a == "--direct");

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Echolet Model Lifecycle Benchmark Tool");
        println!();
        println!("Usage: model_benchmark [OPTIONS]");
        println!();
        println!("Options:");
        println!("  --json         Output benchmark results as JSON");
        println!("  --direct       Run benchmark directly without spawning a child process");
        println!("  --child-fresh  Internal flag for isolated child process invocation");
        println!("  -h, --help     Print help");
        return Ok(());
    }

    if is_child {
        // Run in-process benchmark and output JSON to stdout
        let result = run_in_process_benchmark(3)?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }

    if is_direct {
        let result = run_in_process_benchmark(3)?;
        if is_json {
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            print_benchmark_table(&result);
        }
        return Ok(());
    }

    // Default Parent Process mode: spawn a clean fresh child process to guarantee
    // genuine cold process load and untainted process RSS.
    let exe = env::current_exe()?;
    let output = Command::new(&exe).arg("--child-fresh").output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout_str = String::from_utf8_lossy(&out.stdout);
            // Find the JSON line
            if let Some(json_line) = stdout_str.lines().find(|l| l.starts_with('{')) {
                let result: BenchmarkRunResult = serde_json::from_str(json_line)?;
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    print_benchmark_table(&result);
                }
                Ok(())
            } else {
                eprintln!("[Benchmark Error] Child process did not return expected JSON. Falling back to direct run.");
                let result = run_in_process_benchmark(3)?;
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    print_benchmark_table(&result);
                }
                Ok(())
            }
        }
        _ => {
            // Fallback to direct run if child execution fails
            let result = run_in_process_benchmark(3)?;
            if is_json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print_benchmark_table(&result);
            }
            Ok(())
        }
    }
}
