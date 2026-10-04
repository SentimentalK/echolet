use echolet::diagnostics::benchmark::run_in_process_benchmark;
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
fn test_model_lifecycle_benchmark_smoke() {
    // Observational smoke test: verifies that the benchmark harness compiles, executes the real
    // model lifecycle (ModelManager -> OnlineRecognizer -> OnlineStream -> Unload -> Reload),
    // and produces valid observations without enforcing fragile machine-dependent thresholds.
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
        assert!(f10.warm_total_ms >= 0.0);
    }
}
