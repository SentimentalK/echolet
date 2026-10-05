//! PROJECT-041 Stage 3 / J9 — opt-in, reproducible candidate-model benchmark.
//!
//! This harness measures streaming-ASR candidates against the current Echolet
//! Verified X-ASR baseline. It is deliberately **non-product**: it never reads
//! or writes `models/registry.json`, never marks anything `Echolet Verified`,
//! and never runs as part of ordinary CI. It is a no-op unless explicitly
//! enabled, so the default `cargo test` never downloads a model.
//!
//! Reproducible invocation:
//!
//! ```sh
//! ECHOLET_J9_BENCH=1 \
//! ECHOLET_J9_MODELS_DIR=/path/to/extracted/candidates \
//! ECHOLET_J9_BASELINE_DIR=/path/to/xasr-install \
//! ECHOLET_J9_OUT=/tmp/j9-bench.json \
//! cargo test --test test_j9_candidate_benchmark -- --nocapture
//! ```
//!
//! Candidate identity (archive URL + SHA256 + size) comes from the non-product
//! `models/candidates.lock.json`, so this file contains no weights or audio and
//! no developer-machine absolute paths.
//!
//! Measured per (candidate, probe):
//! * recognizer cold load time (`OnlineRecognizer::from_manifest`);
//! * stream creation time;
//! * process RSS before load / after load / after unload;
//! * first non-empty partial latency while streaming 200 ms chunks;
//! * total decode wall time and real-time factor (RTF);
//! * final transcript for the fixed probe WAV;
//! * unload time (strict J1 order: stream, then recognizer).
//!
//! Quality note: local smoke WAVs are for compatibility/latency sanity only and
//! are **not** a global quality ranking. Upstream-reported WER metrics live in
//! `docs/model-candidates.md`.

use echolet::asr::OnlineRecognizer;
use echolet::diagnostics::memory::{get_current_rss, ProcessRss};
use echolet::models::manifest::ModelManifest;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const SAMPLE_RATE: u32 = 16000;

#[derive(Debug, Deserialize)]
struct CandidateLock {
    #[allow(dead_code)]
    schema_version: u32,
    baseline: CandidateSpec,
    candidates: Vec<CandidateSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct CandidateSpec {
    id: String,
    display_name: String,
    #[serde(default)]
    benchmark: bool,
    #[serde(default)]
    dir_name: Option<String>,
    #[serde(default)]
    default_dir: Option<String>,
    #[serde(default)]
    files: Option<Files>,
    #[serde(default)]
    model_type: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    probes: Vec<Probe>,
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<Source>,
}

#[derive(Debug, Clone, Deserialize)]
struct Files {
    encoder: String,
    decoder: String,
    joiner: String,
    tokens: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Probe {
    name: String,
    wav: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct Source {
    #[serde(default)]
    archive: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ProbeResult {
    candidate_id: String,
    candidate_name: String,
    probe: String,
    wav: String,
    language: Option<String>,
    status: String,
    error: Option<String>,
    recognizer_load_ms: Option<f64>,
    stream_create_ms: Option<f64>,
    rss_before_load_bytes: Option<u64>,
    rss_after_load_bytes: Option<u64>,
    rss_after_unload_bytes: Option<u64>,
    first_partial_ms: Option<f64>,
    decode_ms: Option<f64>,
    audio_secs: Option<f64>,
    rtf: Option<f64>,
    sample_rate: Option<u32>,
    transcript: Option<String>,
    unload_ms: Option<f64>,
}

fn enabled() -> bool {
    matches!(
        std::env::var("ECHOLET_J9_BENCH").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

impl Files {
    fn to_manifest(&self, spec: &CandidateSpec) -> ModelManifest {
        ModelManifest {
            id: spec.id.clone(),
            display_name: spec.display_name.clone(),
            version: "j9-candidate".to_string(),
            languages: spec.languages.clone(),
            family: "online-transducer".to_string(),
            encoder: self.encoder.clone(),
            decoder: self.decoder.clone(),
            joiner: self.joiner.clone(),
            tokens: self.tokens.clone(),
            model_type: spec.model_type.clone(),
            ..Default::default()
        }
    }
}

/// Minimal RIFF/WAVE reader: returns (samples_mono_f32, sample_rate).
///
/// Supports 16-bit PCM and 32-bit float. All J9 probe assets are 16 kHz mono
/// but we read the real header so no implicit assumption leaks in.
fn read_wav_mono_f32(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let bytes = fs::read(path).map_err(|e| format!("read {:?}: {}", path, e))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{:?} is not a RIFF/WAVE file", path));
    }
    let mut pos = 12usize;
    let mut sample_rate = SAMPLE_RATE;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        match id {
            b"fmt " if size >= 16 => {
                let format = u16::from_le_bytes([bytes[body_start], bytes[body_start + 1]]);
                channels = u16::from_le_bytes([bytes[body_start + 2], bytes[body_start + 3]]);
                sample_rate = u32::from_le_bytes([
                    bytes[body_start + 4],
                    bytes[body_start + 5],
                    bytes[body_start + 6],
                    bytes[body_start + 7],
                ]);
                bits = u16::from_le_bytes([bytes[body_start + 14], bytes[body_start + 15]]);
                if format != 1 && format != 3 {
                    return Err(format!("unsupported WAV format tag {}", format));
                }
            }
            b"data" => data = Some(&bytes[body_start..body_end]),
            _ => {}
        }
        pos = body_start + size + (size & 1);
    }
    let data = data.ok_or_else(|| format!("{:?} has no data chunk", path))?;
    let ch = channels.max(1) as usize;

    let mut out: Vec<f32> = Vec::new();
    if bits == 16 {
        let frames = data.len() / (2 * ch);
        out.reserve(frames);
        for f in 0..frames {
            let mut acc = 0.0f32;
            for c in 0..ch {
                let i = (f * ch + c) * 2;
                let s = i16::from_le_bytes([data[i], data[i + 1]]);
                acc += s as f32 / 32768.0;
            }
            out.push(acc / ch as f32);
        }
    } else if bits == 32 {
        let frames = data.len() / (4 * ch);
        out.reserve(frames);
        for f in 0..frames {
            let mut acc = 0.0f32;
            for c in 0..ch {
                let i = (f * ch + c) * 4;
                let s = f32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                acc += s;
            }
            out.push(acc / ch as f32);
        }
    } else {
        return Err(format!("{:?} uses unsupported {}-bit samples", path, bits));
    }
    Ok((out, sample_rate))
}

fn resolve_dir(spec: &CandidateSpec, manifest_dir: &Path, baseline: bool) -> Option<PathBuf> {
    if baseline {
        if let Ok(dir) = std::env::var("ECHOLET_J9_BASELINE_DIR") {
            return Some(PathBuf::from(dir));
        }
    }
    if let Ok(root) = std::env::var("ECHOLET_J9_MODELS_DIR") {
        if let Some(name) = &spec.dir_name {
            return Some(PathBuf::from(root).join(name));
        }
    }
    if let Some(rel) = &spec.default_dir {
        return Some(manifest_dir.join(rel));
    }
    None
}

fn measure(spec: &CandidateSpec, dir: &Path, probe: &Probe) -> ProbeResult {
    let base = ProbeResult {
        candidate_id: spec.id.clone(),
        candidate_name: spec.display_name.clone(),
        probe: probe.name.clone(),
        wav: probe.wav.clone(),
        language: probe.language.clone(),
        status: "pending".to_string(),
        error: None,
        recognizer_load_ms: None,
        stream_create_ms: None,
        rss_before_load_bytes: None,
        rss_after_load_bytes: None,
        rss_after_unload_bytes: None,
        first_partial_ms: None,
        decode_ms: None,
        audio_secs: None,
        rtf: None,
        sample_rate: None,
        transcript: None,
        unload_ms: None,
    };

    let files = match &spec.files {
        Some(f) => f,
        None => {
            return ProbeResult {
                status: "skipped".to_string(),
                error: Some("no files in candidate lock".to_string()),
                ..base
            }
        }
    };
    let wav_path = dir.join(&probe.wav);
    if !wav_path.exists() {
        return ProbeResult {
            status: "skipped".to_string(),
            error: Some(format!("probe wav missing: {}", wav_path.display())),
            ..base
        };
    }

    let manifest = files.to_manifest(spec);
    let (samples, sample_rate) = match read_wav_mono_f32(&wav_path) {
        Ok(v) => v,
        Err(e) => {
            return ProbeResult {
                status: "failed".to_string(),
                error: Some(e),
                ..base
            }
        }
    };

    let rss_before = get_current_rss();

    let t = Instant::now();
    let recognizer = match OnlineRecognizer::from_manifest(dir, &manifest) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            return ProbeResult {
                status: "failed".to_string(),
                error: Some(format!("recognizer init failed: {}", e)),
                rss_before_load_bytes: rss_before.map(|r: ProcessRss| r.bytes()),
                ..base
            }
        }
    };
    let recognizer_load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let rss_after_load = get_current_rss();

    let t = Instant::now();
    let stream = match recognizer.create_stream() {
        Ok(s) => s,
        Err(e) => {
            return ProbeResult {
                status: "failed".to_string(),
                error: Some(format!("stream create failed: {}", e)),
                recognizer_load_ms: Some(recognizer_load_ms),
                rss_before_load_bytes: rss_before.map(|r: ProcessRss| r.bytes()),
                rss_after_load_bytes: rss_after_load.map(|r: ProcessRss| r.bytes()),
                ..base
            }
        }
    };
    let stream_create_ms = t.elapsed().as_secs_f64() * 1000.0;

    let audio_secs = samples.len() as f64 / sample_rate as f64;
    let chunk_size = (sample_rate / 5).max(1) as usize; // ~200 ms at native rate
    let mut first_partial_ms: Option<f64> = None;
    let t_decode = Instant::now();
    for chunk in samples.chunks(chunk_size) {
        stream.accept_waveform(sample_rate as i32, chunk);
        stream.decode_all_ready();
        if first_partial_ms.is_none() {
            let text = stream.get_result();
            if !text.trim().is_empty() {
                first_partial_ms = Some(t_decode.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    let tail = vec![0.0f32; 4800];
    stream.accept_waveform(sample_rate as i32, &tail);
    stream.decode_all_ready();
    let decode_ms = t_decode.elapsed().as_secs_f64() * 1000.0;
    let transcript = stream.get_result();

    drop(stream);
    let t_unload = Instant::now();
    drop(recognizer);
    let unload_ms = t_unload.elapsed().as_secs_f64() * 1000.0;
    let rss_after_unload = get_current_rss();

    ProbeResult {
        status: "ok".to_string(),
        error: None,
        recognizer_load_ms: Some(recognizer_load_ms),
        stream_create_ms: Some(stream_create_ms),
        rss_before_load_bytes: rss_before.map(|r: ProcessRss| r.bytes()),
        rss_after_load_bytes: rss_after_load.map(|r: ProcessRss| r.bytes()),
        rss_after_unload_bytes: rss_after_unload.map(|r: ProcessRss| r.bytes()),
        first_partial_ms,
        decode_ms: Some(decode_ms),
        audio_secs: Some(audio_secs),
        rtf: Some(if audio_secs > 0.0 {
            (decode_ms / 1000.0) / audio_secs
        } else {
            0.0
        }),
        sample_rate: Some(sample_rate),
        transcript: Some(transcript),
        unload_ms: Some(unload_ms),
        ..base
    }
}

#[test]
fn test_j9_candidate_benchmark() {
    if !enabled() {
        eprintln!(
            "[J9] ECHOLET_J9_BENCH is not set; skipping candidate benchmark (CI-safe no-op)."
        );
        return;
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock: CandidateLock = serde_json::from_str(include_str!("../models/candidates.lock.json"))
        .expect("models/candidates.lock.json must parse");

    let mut results: Vec<ProbeResult> = Vec::new();

    // When ECHOLET_J9_ONLY is set, run a single model per process so RSS deltas
    // are not polluted by previously loaded models in the same process.
    let only = std::env::var("ECHOLET_J9_ONLY").unwrap_or_default();
    let run_baseline = only.is_empty() || only == "baseline" || only == lock.baseline.id;

    // Baseline first so the report links candidate deltas to the product default.
    let mut baseline_spec = lock.baseline.clone();
    baseline_spec.benchmark = true;
    if run_baseline {
        if let Some(dir) = resolve_dir(&baseline_spec, manifest_dir, true) {
            for probe in &baseline_spec.probes {
                if !dir.exists() {
                    eprintln!("[J9] baseline dir missing, skipping: {}", dir.display());
                    continue;
                }
                let r = measure(&baseline_spec, &dir, probe);
                eprintln!(
                    "[J9] baseline {} :: {} -> {}",
                    probe.name,
                    r.status,
                    r.transcript.as_deref().unwrap_or("")
                );
                results.push(r);
            }
        }
    }

    for spec in &lock.candidates {
        if !spec.benchmark {
            continue;
        }
        if !only.is_empty() && spec.id != only {
            continue;
        }
        let dir = match resolve_dir(spec, manifest_dir, false) {
            Some(d) => d,
            None => continue,
        };
        if !dir.exists() {
            eprintln!(
                "[J9] candidate {} dir missing (not downloaded), skipping: {}",
                spec.id,
                dir.display()
            );
            continue;
        }
        for probe in &spec.probes {
            let r = measure(spec, &dir, probe);
            eprintln!(
                "[J9] {} :: {} -> {} ({})",
                spec.id,
                probe.name,
                r.status,
                r.transcript.as_deref().unwrap_or("")
            );
            results.push(r);
        }
    }

    let json = serde_json::to_string_pretty(&results).expect("serialize results");
    if let Ok(out) = std::env::var("ECHOLET_J9_OUT") {
        let _ = fs::write(&out, &json);
        eprintln!("[J9] wrote {} result rows to {}", results.len(), out);
    }
    println!("J9_RESULT_JSON_BEGIN");
    println!("{}", json);
    println!("J9_RESULT_JSON_END");

    let ok = results.iter().filter(|r| r.status == "ok").count();
    assert!(
        ok > 0,
        "J9 benchmark enabled but no (candidate, probe) pair succeeded"
    );
}
