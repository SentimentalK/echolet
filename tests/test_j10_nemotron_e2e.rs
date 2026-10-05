//! PROJECT-041 Stage 3 / J10 — opt-in, reproducible end-to-end verification of
//! the Echolet-owned frozen Nemotron 3.5 ASR Streaming 0.6B (560 ms, int8) pack.
//!
//! Ordinary CI must not touch the network or download ~450 MB, so this harness
//! is a CI-safe no-op unless explicitly enabled:
//!
//! ```sh
//! ECHOLET_J10_E2E=1 cargo test --test test_j10_nemotron_e2e -- --nocapture
//! ```
//!
//! It performs a real product-path run:
//!   1. clean NO_MODEL temp user root + shipped registry;
//!   2. install the frozen Echolet-owned pack via the J6 product downloader
//!      (checksum enforced) from the Echolet Release URL, with no auto-select;
//!   3. explicit selection of the Nemotron pack;
//!   4. real recognizer/stream creation from the freshly installed files;
//!   5. forced-language Japanese via the new core `OnlineStream` API set BEFORE
//!      any audio, and a language sanity check proving the J9 Korean-ish
//!      auto-detect failure mode is gone;
//!   6. an auto-detect smoke on a known out-of-box language;
//!   7. product rejection of an adaptation-ready locale before inference.
//!
//! WAV acquisition (the frozen pack intentionally ships no test audio):
//!   * `ECHOLET_J10_JA_WAV` / `ECHOLET_J10_ZH_WAV` point at local copies; their
//!     SHA256 is still verified against `models/nemotron-model.lock.json`.
//!   * otherwise the pinned upstream archive from `models/nemotron-model.json`
//!     is downloaded, its SHA256 verified, and `test_wavs/{ja,zh}.wav` extracted.
//!   * `ECHOLET_J10_UPSTREAM_ARCHIVE` reuses an already-downloaded upstream
//!     `.tar.bz2` (its SHA256 is still verified).

use echolet::asr::OnlineRecognizer;
use echolet::models::download::InstallPhase;
use echolet::models::manager::ModelManager;
use echolet::models::registry::{ModelRegistry, VerificationStatus};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const NEMOTRON_ID: &str = "echolet-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11-r1";
const XASR_ID: &str = "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1";

fn e2e_enabled() -> bool {
    matches!(
        std::env::var("ECHOLET_J10_E2E").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn unique_tmp_root() -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("echolet-j10-e2e-{}-{}", std::process::id(), now))
}

#[derive(Debug)]
struct Authorities {
    registry: ModelRegistry,
    lock: serde_json::Value,
    def: serde_json::Value,
}

fn load_authorities() -> Result<Authorities, String> {
    let registry = ModelRegistry::from_str(include_str!("../models/registry.json"))
        .map_err(|e| format!("shipped registry must parse: {}", e))?;
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.lock.json"))
            .map_err(|e| format!("nemotron lock must parse: {}", e))?;
    let def: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.json"))
            .map_err(|e| format!("nemotron definition must parse: {}", e))?;
    Ok(Authorities {
        registry,
        lock,
        def,
    })
}

fn download(url: &str, dest: &Path, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|e| format!("download {} failed: {}", url, e))?;
    let mut buf = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| format!("read body for {} failed: {}", url, e))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
    }
    fs::write(dest, &buf).map_err(|e| format!("write {:?}: {}", dest, e))?;
    Ok(buf)
}

/// Extracts `test_wavs/<name>.wav` from the pinned upstream `.tar.bz2`.
fn wav_from_upstream_archive(archive: &Path, wav_name: &str) -> Result<Vec<u8>, String> {
    let file = fs::File::open(archive).map_err(|e| format!("open upstream archive: {}", e))?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|e| format!("read upstream archive entries: {}", e))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("read upstream entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("entry path: {}", e))?
            .to_path_buf();
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if file_name == wav_name {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("read {}: {}", wav_name, e))?;
            return Ok(buf);
        }
    }
    Err(format!(
        "{} not found in upstream archive {:?}",
        wav_name, archive
    ))
}

/// Ensures the pinned upstream archive is available locally, without
/// re-downloading when `ECHOLET_J10_UPSTREAM_ARCHIVE` is provided.
fn acquire_upstream_archive(def: &serde_json::Value, work: &Path) -> Result<PathBuf, String> {
    let expected_sha = def["upstream_source"]["sha256"]
        .as_str()
        .ok_or("definition missing upstream sha256")?;
    let url = def["upstream_source"]["url"]
        .as_str()
        .ok_or("definition missing upstream url")?;

    if let Ok(local) = std::env::var("ECHOLET_J10_UPSTREAM_ARCHIVE") {
        let bytes = fs::read(&local).map_err(|e| format!("read {}: {}", local, e))?;
        if sha256_hex(&bytes) != expected_sha {
            return Err(format!("upstream archive SHA mismatch for {}", local));
        }
        return Ok(PathBuf::from(local));
    }

    let dest = work.join("upstream.tar.bz2");
    eprintln!("[J10] downloading pinned upstream archive {}", url);
    let bytes = download(url, &dest, 1800)?;
    if sha256_hex(&bytes) != expected_sha {
        return Err(format!("pinned upstream archive SHA mismatch for {}", url));
    }
    Ok(dest)
}

fn acquire_wav(
    wav_name: &str,
    env_var: &str,
    expected_sha: &str,
    upstream_archive: &Path,
    work: &Path,
) -> Result<PathBuf, String> {
    let bytes = if let Ok(local) = std::env::var(env_var) {
        eprintln!("[J10] reading {} from {}", wav_name, local);
        fs::read(&local).map_err(|e| format!("read {}: {}", local, e))?
    } else {
        wav_from_upstream_archive(upstream_archive, wav_name)?
    };
    let observed = sha256_hex(&bytes);
    if observed != expected_sha {
        return Err(format!(
            "{} SHA256 mismatch: expected {}, got {}",
            wav_name, expected_sha, observed
        ));
    }
    let path = work.join(wav_name);
    fs::write(&path, &bytes).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(path)
}

/// Minimal RIFF/WAVE reader: returns (mono f32 samples, sample rate).
fn read_wav_mono_f32(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let bytes = fs::read(path).map_err(|e| format!("read {:?}: {}", path, e))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{:?} is not a RIFF/WAVE file", path));
    }
    let mut pos = 12usize;
    let mut sample_rate = 16000u32;
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
                channels = u16::from_le_bytes([bytes[body_start + 2], bytes[body_start + 3]]);
                sample_rate = u32::from_le_bytes([
                    bytes[body_start + 4],
                    bytes[body_start + 5],
                    bytes[body_start + 6],
                    bytes[body_start + 7],
                ]);
                bits = u16::from_le_bytes([bytes[body_start + 14], bytes[body_start + 15]]);
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
        for f in 0..frames {
            let mut acc = 0.0f32;
            for c in 0..ch {
                let i = (f * ch + c) * 4;
                acc += f32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            }
            out.push(acc / ch as f32);
        }
    } else {
        return Err(format!("{:?} uses unsupported {}-bit samples", path, bits));
    }
    Ok((out, sample_rate))
}

/// Decodes a whole WAV through a stream, optionally forcing a language first.
fn transcribe(
    recognizer: &Arc<OnlineRecognizer>,
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<(String, bool), String> {
    let stream = recognizer
        .create_stream()
        .map_err(|e| format!("stream creation failed: {}", e))?;
    // Forced language MUST be set before any audio is accepted.
    let mut option_applied = false;
    if let Some(code) = language {
        stream
            .set_language(Some(code))
            .map_err(|e| format!("set_language({:?}) failed: {}", code, e))?;
        option_applied = true;
    }
    let chunk = (sample_rate / 5).max(1) as usize;
    for c in samples.chunks(chunk) {
        stream.accept_waveform(sample_rate as i32, c);
        stream.decode_all_ready();
    }
    let tail = vec![0.0f32; 4800];
    stream.accept_waveform(sample_rate as i32, &tail);
    stream.decode_all_ready();
    Ok((stream.get_result(), option_applied))
}

fn has_japanese(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

fn has_hangul(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{ac00}'..='\u{d7af}').contains(&c) || ('\u{1100}'..='\u{11ff}').contains(&c))
}

fn is_sane(text: &str) -> bool {
    text.trim().chars().filter(|c| !c.is_whitespace()).count() >= 2
}

fn installed_pack_size(dir: &Path, manifest: &echolet::models::manifest::ModelManifest) -> u64 {
    [
        &manifest.encoder,
        &manifest.decoder,
        &manifest.joiner,
        &manifest.tokens,
    ]
    .iter()
    .filter_map(|n| fs::metadata(dir.join(n)).ok())
    .map(|m| m.len())
    .sum()
}

#[test]
fn test_j10_nemotron_e2e() {
    if !e2e_enabled() {
        eprintln!(
            "[J10] ECHOLET_J10_E2E is not set; skipping real-network verification (CI-safe)."
        );
        return;
    }
    let result = run_e2e();
    if let Err(err) = &result {
        eprintln!("[J10] FAILED: {}", err);
    }
    assert!(result.is_ok(), "J10 E2E failed: {:?}", result.err());
}

fn run_e2e() -> Result<(), String> {
    let auth = load_authorities()?;

    let entry = auth
        .registry
        .get_model(NEMOTRON_ID)
        .ok_or("registry missing Nemotron entry")?
        .clone();

    // ---- Identity checks against the frozen Echolet release lock ----
    if entry.source.url.as_deref() != auth.lock["url"].as_str() {
        return Err("registry Nemotron url != lock url".into());
    }
    if entry.source.sha256.as_deref() != auth.lock["sha256"].as_str() {
        return Err("registry Nemotron sha256 != lock sha256".into());
    }
    if auth.registry.default_model_id != XASR_ID {
        return Err("X-ASR must remain the default model".into());
    }
    let xasr = auth.registry.get_model(XASR_ID).ok_or("missing X-ASR")?;
    if xasr.verification_status != VerificationStatus::EcholetVerified {
        return Err("X-ASR must remain Echolet Verified".into());
    }
    eprintln!(
        "[J10] nemotron_release_url={}",
        entry.source.url.as_deref().unwrap_or("")
    );
    eprintln!(
        "[J10] nemotron_archive_sha={}",
        entry.source.sha256.as_deref().unwrap_or("")
    );

    // ---- Clean temp NO_MODEL root ----
    let root = unique_tmp_root();
    let bundled = root.join("bundled");
    let user = root.join("user");
    let config = root.join("config.json");
    let work = root.join("work");
    fs::create_dir_all(&bundled).map_err(|e| format!("mkdir bundled: {}", e))?;
    fs::create_dir_all(&user).map_err(|e| format!("mkdir user: {}", e))?;
    fs::create_dir_all(&work).map_err(|e| format!("mkdir work: {}", e))?;
    fs::write(
        bundled.join("registry.json"),
        include_str!("../models/registry.json"),
    )
    .map_err(|e| format!("write bundled registry: {}", e))?;

    let cleanup = |r: &Path| {
        let _ = fs::remove_dir_all(r);
    };

    let run = (|| -> Result<(), String> {
        let mut manager =
            ModelManager::new_with_paths(bundled.clone(), user.clone(), config.clone())?;
        if !manager.installed.is_empty() {
            return Err("expected clean NO_MODEL state".into());
        }

        // ---- Product install (real network, checksum enforced) ----
        let mut advertised_total: Option<u64> = None;
        let mut observed_downloaded: u64 = 0;
        {
            let mut on_progress = |phase: InstallPhase| {
                if let InstallPhase::Downloading {
                    downloaded_bytes,
                    total_bytes,
                } = phase
                {
                    observed_downloaded = observed_downloaded.max(downloaded_bytes);
                    if let Some(t) = total_bytes {
                        advertised_total = Some(t);
                    }
                }
            };
            manager
                .install_registry_model(NEMOTRON_ID, Some(&mut on_progress))
                .map_err(|e| format!("product install failed: {}", e))?;
        }

        // ---- No auto-select; explicit select works ----
        if manager.active_model_id().is_some() {
            return Err("install must not auto-select a model".into());
        }
        manager
            .set_active_model(NEMOTRON_ID)
            .map_err(|e| format!("explicit select failed: {}", e))?;
        if manager.active_model_id() != Some(NEMOTRON_ID) {
            return Err("explicit select did not take effect".into());
        }

        let installed = manager
            .get_model(NEMOTRON_ID)
            .ok_or("installed model missing")?
            .clone();
        installed
            .manifest
            .validate_files(&installed.dir)
            .map_err(|e| format!("installed manifest invalid: {}", e))?;

        let download_size = advertised_total.unwrap_or(observed_downloaded);
        let installed_size = installed_pack_size(&installed.dir, &installed.manifest);
        eprintln!("[J10] downloaded_bytes={}", observed_downloaded);
        eprintln!("[J10] download_size_bytes={}", download_size);
        eprintln!("[J10] installed_size_bytes={}", installed_size);
        eprintln!("[J10] installed_path={}", installed.dir.display());
        if entry.download_size_bytes != Some(download_size) {
            return Err(format!(
                "registry download_size_bytes {:?} != observed {}",
                entry.download_size_bytes, download_size
            ));
        }
        if entry.installed_size_bytes != Some(installed_size) {
            return Err(format!(
                "registry installed_size_bytes {:?} != observed {}",
                entry.installed_size_bytes, installed_size
            ));
        }

        // ---- Product selection contract: adaptation-ready rejected pre-inference ----
        for a in entry.adaptation_ready_locales() {
            if entry.validate_language_selection(Some(&a.locale)).is_ok() {
                return Err(format!(
                    "adaptation-ready locale {} must be rejected",
                    a.locale
                ));
            }
        }
        let ja_option = entry
            .validate_language_selection(Some("ja-JP"))
            .map_err(|e| format!("ja-JP must be selectable: {}", e))?
            .ok_or("ja-JP lookup returned none")?;
        eprintln!(
            "[J10] ja locale={} runtime_code={} tier={}",
            ja_option.locale,
            ja_option.runtime_code,
            ja_option.tier.label()
        );

        // ---- Acquire pinned verification WAVs ----
        let upstream_archive = acquire_upstream_archive(&auth.def, &work)?;
        let ja_sha = auth.def["verification_wavs"]["ja"]["sha256"]
            .as_str()
            .unwrap();
        let zh_sha = auth.def["verification_wavs"]["zh"]["sha256"]
            .as_str()
            .unwrap();
        let ja_wav = acquire_wav(
            "ja.wav",
            "ECHOLET_J10_JA_WAV",
            ja_sha,
            &upstream_archive,
            &work,
        )?;
        let zh_wav = acquire_wav(
            "zh.wav",
            "ECHOLET_J10_ZH_WAV",
            zh_sha,
            &upstream_archive,
            &work,
        )?;
        eprintln!("[J10] ja_wav_sha={}", ja_sha);
        eprintln!("[J10] zh_wav_sha={}", zh_sha);

        let (ja_samples, ja_rate) = read_wav_mono_f32(&ja_wav)?;
        let (zh_samples, zh_rate) = read_wav_mono_f32(&zh_wav)?;
        eprintln!("[J10] ja_wav_rate={} zh_wav_rate={}", ja_rate, zh_rate);

        // ---- Real recognizer/stream ----
        let recognizer = Arc::new(
            OnlineRecognizer::from_manifest(&installed.dir, &installed.manifest)
                .map_err(|e| format!("recognizer init failed: {}", e))?,
        );

        // ---- Forced Japanese via the new core API (set BEFORE audio) ----
        let (ja_transcript, applied) = transcribe(
            &recognizer,
            &ja_samples,
            ja_rate,
            Some(ja_option.runtime_code.as_str()),
        )?;
        if !applied {
            return Err("forced language option was not applied".into());
        }
        eprintln!("[J10] forced_ja_transcript={}", ja_transcript);
        if !is_sane(&ja_transcript) {
            return Err(format!(
                "forced Japanese transcript empty/insane: {:?}",
                ja_transcript
            ));
        }
        if !has_japanese(&ja_transcript) {
            return Err(format!(
                "forced Japanese transcript has no Japanese characters: {:?}",
                ja_transcript
            ));
        }
        if has_hangul(&ja_transcript) {
            return Err(format!(
                "forced Japanese transcript still contains Hangul (J9 failure mode): {:?}",
                ja_transcript
            ));
        }

        // ---- Auto-detect smoke on a known out-of-box language (zh) ----
        let (auto_transcript, _) = transcribe(&recognizer, &zh_samples, zh_rate, None)?;
        eprintln!("[J10] auto_zh_transcript={}", auto_transcript);
        if !is_sane(&auto_transcript) {
            return Err(format!(
                "auto-detect transcript empty/insane: {:?}",
                auto_transcript
            ));
        }

        // ---- Optional X-ASR regression smoke ----
        if let Ok(xasr_dir) = std::env::var("ECHOLET_J10_XASR_DIR") {
            if let Ok(xasr_wav) = std::env::var("ECHOLET_J10_XASR_WAV") {
                let xasr_manifest = echolet::models::manifest::ModelManifest::from_file(
                    &Path::new(&xasr_dir).join("model.json"),
                )?;
                let xasr_rec = Arc::new(
                    OnlineRecognizer::from_manifest(&xasr_dir, &xasr_manifest)
                        .map_err(|e| format!("X-ASR recognizer init failed: {}", e))?,
                );
                let (samples, rate) = read_wav_mono_f32(Path::new(&xasr_wav))?;
                let (t, _) = transcribe(&xasr_rec, &samples, rate, None)?;
                eprintln!("[J10] xasr_transcript={}", t);
                if !is_sane(&t) {
                    return Err("X-ASR regression smoke produced empty/insane transcript".into());
                }
            }
        }

        eprintln!("[J10] PASS: frozen pack install + forced Japanese + auto smoke verified");
        Ok(())
    })();

    cleanup(&root);
    run
}
