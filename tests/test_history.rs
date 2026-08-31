use chrono::{Duration, TimeZone, Utc};
use echolet::config::EcholetConfig;
use echolet::history::{HistoryEntry, HistoryManager};
use std::fs;
use std::path::PathBuf;

fn test_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("echolet-test-{}", name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_config_defaults_and_persistence() {
    let temp_dir = test_temp_dir("config");
    let cfg_path = temp_dir.join("config.json");

    // 1. Loading from non-existent file yields defaults
    let default_cfg = EcholetConfig::load_from(&cfg_path);
    assert!(!default_cfg.history_enabled);
    assert!(!default_cfg.selected_model.is_empty());

    // 2. Mutate and save
    let mut modified_cfg = default_cfg;
    modified_cfg.history_enabled = true;
    modified_cfg.selected_model = "test-custom-model-id".to_string();
    modified_cfg.save_to(&cfg_path).unwrap();

    // 3. Reload from disk and verify persistence
    let reloaded_cfg = EcholetConfig::load_from(&cfg_path);
    assert!(reloaded_cfg.history_enabled);
    assert_eq!(reloaded_cfg.selected_model, "test-custom-model-id");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_history_disabled_by_default_writes_nothing() {
    let temp_dir = test_temp_dir("disabled");
    let mut manager = HistoryManager::new(false, temp_dir.clone());

    let t0 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap().into();
    let t1 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 10).unwrap().into();

    manager.on_utterance(t0, t1, "Hello world this is test", "test-model");
    let flushed = manager.flush();

    assert!(flushed.is_none());
    assert!(!temp_dir.join("2026-08-31.jsonl").exists());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_history_aggregation_under_60_seconds_gap() {
    let temp_dir = test_temp_dir("gap-under-60");
    let mut manager = HistoryManager::new(true, temp_dir.clone());

    // Utterance 1: 10:00:00 -> 10:00:10
    let t0 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap().into();
    let t1 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 10).unwrap().into();
    manager.on_utterance(t0, t1, "我觉得这个功能很实用。", "test-model-xasr");

    // Utterance 2: 10:00:30 -> 10:00:40 (gap = 20s < 60s)
    let t2 = t1 + Duration::seconds(20);
    let t3 = t2 + Duration::seconds(10);
    manager.on_utterance(t2, t3, "然后把数据存在本地。", "test-model-xasr");

    // Utterance 3: 10:01:39 -> 10:01:45 (gap = 59s < 60s)
    let t4 = t3 + Duration::seconds(59);
    let t5 = t4 + Duration::seconds(6);
    manager.on_utterance(t4, t5, "没有网络也完全可用。", "test-model-xasr");

    // Flush and verify
    let entry = manager.flush().expect("Expected flushed entry");
    assert_eq!(
        entry.text,
        "我觉得这个功能很实用。然后把数据存在本地。没有网络也完全可用。"
    );
    assert_eq!(entry.model, "test-model-xasr");
    assert_eq!(entry.start, t0.to_rfc3339());
    assert_eq!(entry.end, t5.to_rfc3339());

    // Verify written JSONL line
    let jsonl_path = temp_dir.join("2026-08-31.jsonl");
    let content = fs::read_to_string(&jsonl_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 1);

    let parsed: HistoryEntry = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed, entry);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_history_segmentation_at_or_above_60_seconds_gap() {
    let temp_dir = test_temp_dir("gap-over-60");
    let mut manager = HistoryManager::new(true, temp_dir.clone());

    // Utterance 1: 10:00:00 -> 10:00:10
    let t0 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap().into();
    let t1 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 10).unwrap().into();
    manager.on_utterance(t0, t1, "First paragraph text.", "test-model");

    // Utterance 2: 10:01:10 -> 10:01:20 (gap = 60s >= 60s) -> triggers auto flush of #1 and starts #2
    let t2 = t1 + Duration::seconds(60);
    let t3 = t2 + Duration::seconds(10);
    manager.on_utterance(t2, t3, "Second paragraph text.", "test-model");

    // Utterance 3: 10:06:20 -> 10:06:30 (gap = 300s / 5min >= 60s) -> triggers auto flush of #2 and starts #3
    let t4 = t3 + Duration::seconds(300);
    let t5 = t4 + Duration::seconds(10);
    manager.on_utterance(t4, t5, "Third paragraph text.", "test-model");

    // Flush final #3
    manager.flush();

    let jsonl_path = temp_dir.join("2026-08-31.jsonl");
    let content = fs::read_to_string(&jsonl_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 3, "Must have exactly 3 separate JSONL lines");

    let e1: HistoryEntry = serde_json::from_str(lines[0]).unwrap();
    let e2: HistoryEntry = serde_json::from_str(lines[1]).unwrap();
    let e3: HistoryEntry = serde_json::from_str(lines[2]).unwrap();

    assert_eq!(e1.text, "First paragraph text.");
    assert_eq!(e2.text, "Second paragraph text.");
    assert_eq!(e3.text, "Third paragraph text.");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_history_toggle_off_flushes_pending_draft() {
    let temp_dir = test_temp_dir("toggle-off");
    let mut manager = HistoryManager::new(true, temp_dir.clone());

    let t0 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap().into();
    let t1 = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 10).unwrap().into();
    manager.on_utterance(t0, t1, "Text before disable.", "test-model");

    assert!(manager.current_draft().is_some());

    // Toggle off
    manager.set_enabled(false);
    assert!(manager.current_draft().is_none());

    // Verify it was written to disk
    let jsonl_path = temp_dir.join("2026-08-31.jsonl");
    assert!(jsonl_path.exists());
    let content = fs::read_to_string(&jsonl_path).unwrap();
    assert!(content.contains("Text before disable."));

    // Further utterances while disabled do nothing
    let t2 = t1 + Duration::seconds(5);
    let t3 = t2 + Duration::seconds(5);
    manager.on_utterance(t2, t3, "Ignored text while disabled.", "test-model");
    let content_after = fs::read_to_string(&jsonl_path).unwrap();
    assert!(!content_after.contains("Ignored text"));

    let _ = fs::remove_dir_all(&temp_dir);
}
