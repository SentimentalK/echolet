use echolet::diagnostics::benchmark::{
    build_child_args, is_benchmark_invocation, run_in_process_benchmark,
};
use echolet::diagnostics::memory::{
    get_current_rss, parse_statm_rss_pages, parse_status_vm_rss_bytes, ProcessRss,
};

#[test]
fn test_statm_rss_parser_edge_cases() {
    // Normal single-space
    assert_eq!(parse_statm_rss_pages("100 250 50 10 0 20 0"), Some(250));
    // Tab and multi-space separation
    assert_eq!(
        parse_statm_rss_pages("  100\t\t500   50 10 0 20 0 \n"),
        Some(500)
    );
    // Empty or malformed
    assert_eq!(parse_statm_rss_pages(""), None);
    assert_eq!(parse_statm_rss_pages("not_a_number"), None);
    assert_eq!(parse_statm_rss_pages("100 not_a_number 50"), None);
}

#[test]
fn test_status_vm_rss_parser_edge_cases() {
    let mock_status = "\
Name:\techolet\n\
State:\tS (sleeping)\n\
VmPeak:\t  120000 kB\n\
VmSize:\t  100000 kB\n\
VmRSS:\t   65536 kB\n\
VmHWM:\t   70000 kB\n";

    assert_eq!(parse_status_vm_rss_bytes(mock_status), Some(65536 * 1024));

    // Missing VmRSS
    assert_eq!(parse_status_vm_rss_bytes("Name:\techolet\n"), None);

    // Unit parsing (MB, B)
    assert_eq!(
        parse_status_vm_rss_bytes("VmRSS: 128 MB\n"),
        Some(128 * 1024 * 1024)
    );
    assert_eq!(parse_status_vm_rss_bytes("VmRSS: 2048 B\n"), Some(2048));
}

#[test]
fn test_process_rss_units() {
    let rss = ProcessRss::new(50 * 1024 * 1024);
    assert_eq!(rss.bytes(), 52428800);
    assert_eq!(rss.kib(), 51200.0);
    assert_eq!(rss.mib(), 50.0);
}

#[test]
fn test_get_current_rss_smoke() {
    let rss = get_current_rss();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        assert!(rss.is_some(), "RSS should be supported on Linux and macOS");
        assert!(rss.unwrap().bytes() > 0, "RSS must be > 0 bytes");
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = rss;
    }
}

#[test]
fn test_child_args_construction() {
    // 1. From main binary invocation (e.g. echolet benchmark / bench / --benchmark)
    let args_bench = vec!["echolet".to_string(), "benchmark".to_string()];
    assert_eq!(
        build_child_args(&args_bench),
        vec!["benchmark", "--child-fresh"]
    );

    let args_bench_short = vec![
        "echolet".to_string(),
        "bench".to_string(),
        "--json".to_string(),
    ];
    assert_eq!(
        build_child_args(&args_bench_short),
        vec!["bench", "--child-fresh"]
    );

    let args_bench_flag = vec![
        "/usr/local/bin/echolet".to_string(),
        "--benchmark".to_string(),
    ];
    assert_eq!(
        build_child_args(&args_bench_flag),
        vec!["--benchmark", "--child-fresh"]
    );

    // 2. From dedicated binary invocation (e.g. model_benchmark)
    let args_dedicated = vec!["model_benchmark".to_string()];
    assert_eq!(build_child_args(&args_dedicated), vec!["--child-fresh"]);

    let args_dedicated_json = vec![
        "target/release/model_benchmark".to_string(),
        "--json".to_string(),
    ];
    assert_eq!(
        build_child_args(&args_dedicated_json),
        vec!["--child-fresh"]
    );
}

#[test]
fn test_benchmark_cli_routing() {
    // 1. Valid benchmark entrypoints that should route into benchmark CLI
    assert!(is_benchmark_invocation(&[
        "echolet".into(),
        "benchmark".into()
    ]));
    assert!(is_benchmark_invocation(&["echolet".into(), "bench".into()]));
    assert!(is_benchmark_invocation(&[
        "echolet".into(),
        "--benchmark".into()
    ]));

    // 2. Internal child-fresh flag must also be captured so child invocation never
    // falls through into normal background daemon self-detach or GUI startup.
    assert!(is_benchmark_invocation(&[
        "echolet".into(),
        "--child-fresh".into()
    ]));

    // 3. Regular non-benchmark subcommands or flags must not be intercepted
    assert!(!is_benchmark_invocation(&["echolet".into()]));
    assert!(!is_benchmark_invocation(&[
        "echolet".into(),
        "toggle".into()
    ]));
    assert!(!is_benchmark_invocation(&["echolet".into(), "stop".into()]));
    assert!(!is_benchmark_invocation(&[
        "echolet".into(),
        "--foreground".into()
    ]));
}

#[test]
fn test_model_lifecycle_benchmark_smoke() {
    // Ordinary CI validates benchmark harness compilation and unit logic cheaply.
    // Heavy real 500MB model loading / multi-cycle benchmark is explicitly opt-in
    // (via ECHOLET_BENCHMARK_SMOKE=1) so normal CI runs across Linux, macOS, and Windows
    // are not materially inflated.
    let opt_in = std::env::var("ECHOLET_BENCHMARK_SMOKE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !opt_in {
        println!(
            "[Notice] Skipping heavy model lifecycle smoke test during ordinary cargo test. Set ECHOLET_BENCHMARK_SMOKE=1 to run."
        );
        return;
    }

    let result =
        run_in_process_benchmark(1).expect("Benchmark smoke run must succeed using active model");

    assert!(!result.model_id.is_empty(), "Model ID must be populated");
    assert!(
        !result.model_name.is_empty(),
        "Model Name must be populated"
    );
    assert!(
        result.fresh_load_recog_ms > 0.0,
        "Fresh recognizer load duration must be > 0"
    );
    assert!(
        result.fresh_load_stream_ms > 0.0,
        "Fresh stream creation duration must be > 0"
    );
    assert!(
        result.fresh_load_total_ms > 0.0,
        "Fresh total load duration must be > 0"
    );
    assert!(
        result.unload_ms >= 0.0,
        "Unload duration must be non-negative"
    );
    assert!(
        result.immediate_reload_recog_ms > 0.0,
        "Immediate reload recognizer duration must be > 0"
    );
    assert!(
        result.immediate_reload_stream_ms > 0.0,
        "Immediate reload stream duration must be > 0"
    );
    assert_eq!(
        result.warm_cycles.len(),
        1,
        "Expected 1 warm cycle in smoke test"
    );
    assert!(result.warm_cycles[0].unload_ms >= 0.0);
    assert!(result.warm_cycles[0].total_reload_ms > 0.0);

    if let Some(f10) = result.f10_latency {
        assert!(f10.unloaded_total_ms > 0.0);
        assert!(f10.unloaded_model_ready_ms > 0.0);
        assert!(f10.warm_total_ms >= 0.0);
        assert!(f10.warm_model_ready_ms >= 0.0);
    }
}
