//! Opt-in install, load, and inference check for the English catalog models.
//!
//! Ordinary CI must not download the archives. Enable it explicitly:
//!
//! ```sh
//! ECHOLET_ENGLISH_MODEL_E2E=1 cargo test --release --test test_english_models_e2e -- --nocapture
//! ```
//!
//! The harness uses the shipped registry and `ModelManager::install_registry_model`,
//! which downloads each archive from its registry URL and verifies the SHA256
//! before install. A shared English WAV from the official Kroko archive is
//! then decoded by each installed English recognizer.

use echolet::asr::OnlineRecognizer;
use echolet::models::download::InstallPhase;
use echolet::models::manager::ModelManager;
use echolet::models::manifest::ModelManifest;
use echolet::models::registry::ModelRegistry;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const KROKO_ID: &str = "echolet-kroko-streaming-en-2025-08-06-r1";
const NEMOTRON_ID: &str = "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1";
const PARAKEET_ID: &str = "echolet-parakeet-unified-en-0.6b-560ms-int8-2026-05-12-r1";

fn e2e_enabled() -> bool {
    matches!(
        std::env::var("ECHOLET_ENGLISH_MODEL_E2E").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unique_tmp_root() -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "echolet-english-e2e-{}-{}",
        std::process::id(),
        now
    ))
}

fn installed_pack_size(dir: &Path, manifest: &ModelManifest) -> u64 {
    [
        &manifest.encoder,
        &manifest.decoder,
        &manifest.joiner,
        &manifest.tokens,
    ]
    .iter()
    .map(|name| {
        fs::metadata(dir.join(name))
            .map(|meta| meta.len())
            .unwrap_or(0)
    })
    .sum()
}

/// Minimal RIFF/WAVE reader for 16-bit or 32-bit PCM. Returns mono f32 samples.
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
    let mut out = Vec::new();
    if bits == 16 {
        let frames = data.len() / (2 * ch);
        for frame in 0..frames {
            let mut acc = 0.0f32;
            for c in 0..ch {
                let i = (frame * ch + c) * 2;
                let sample = i16::from_le_bytes([data[i], data[i + 1]]);
                acc += sample as f32 / 32768.0;
            }
            out.push(acc / ch as f32);
        }
    } else if bits == 32 {
        let frames = data.len() / (4 * ch);
        for frame in 0..frames {
            let mut acc = 0.0f32;
            for c in 0..ch {
                let i = (frame * ch + c) * 4;
                acc += f32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            }
            out.push(acc / ch as f32);
        }
    } else {
        return Err(format!("{:?} uses unsupported {}-bit samples", path, bits));
    }
    Ok((out, sample_rate))
}

fn transcribe(recognizer: &Arc<OnlineRecognizer>, samples: &[f32], sample_rate: u32) -> String {
    let stream = recognizer.create_stream().expect("stream creation failed");
    let chunk = (sample_rate / 5).max(1) as usize;
    for piece in samples.chunks(chunk) {
        stream.accept_waveform(sample_rate as i32, piece);
        stream.decode_all_ready();
    }
    let tail = vec![0.0f32; sample_rate as usize];
    stream.accept_waveform(sample_rate as i32, &tail);
    stream.decode_all_ready();
    stream.get_result()
}

fn install_model(manager: &mut ModelManager, model_id: &str) -> Result<(), String> {
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
                if let Some(total) = total_bytes {
                    advertised_total = Some(total);
                }
            }
        };
        manager
            .install_registry_model(model_id, Some(&mut on_progress))
            .map_err(|e| format!("product install failed for {}: {}", model_id, e))?;
    }

    let entry = manager
        .registry
        .get_model(model_id)
        .ok_or_else(|| format!("registry missing {}", model_id))?
        .clone();
    let installed = manager
        .get_model(model_id)
        .ok_or_else(|| format!("installed model missing: {}", model_id))?
        .clone();
    installed
        .manifest
        .validate_files(&installed.dir)
        .map_err(|e| format!("installed files missing for {}: {}", model_id, e))?;

    for name in [
        &installed.manifest.encoder,
        &installed.manifest.decoder,
        &installed.manifest.joiner,
        &installed.manifest.tokens,
    ] {
        let path = installed.dir.join(name);
        let len = fs::metadata(&path)
            .map(|meta| meta.len())
            .map_err(|e| format!("stat {:?}: {}", path, e))?;
        if len == 0 {
            return Err(format!("installed file is empty: {}", path.display()));
        }
        eprintln!("[english-e2e] file {} bytes={}", path.display(), len);
    }

    let download_size = advertised_total.unwrap_or(observed_downloaded);
    let installed_size = installed_pack_size(&installed.dir, &installed.manifest);
    eprintln!(
        "[english-e2e] model={} download_size_bytes={} installed_size_bytes={}",
        model_id, download_size, installed_size
    );
    if entry.download_size_bytes != Some(download_size) {
        return Err(format!(
            "registry download_size_bytes {:?} != observed {} for {}",
            entry.download_size_bytes, download_size, model_id
        ));
    }
    if entry.installed_size_bytes != Some(installed_size) {
        return Err(format!(
            "registry installed_size_bytes {:?} != observed {} for {}",
            entry.installed_size_bytes, installed_size, model_id
        ));
    }
    Ok(())
}

fn run_e2e() -> Result<(), String> {
    let registry = ModelRegistry::from_str(include_str!("../models/registry.json"))
        .map_err(|e| format!("shipped registry must parse: {}", e))?;
    let ids: Vec<&str> = registry.models.iter().map(|m| m.id.as_str()).collect();
    if ids
        != [
            "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
            KROKO_ID,
            NEMOTRON_ID,
            PARAKEET_ID,
        ]
    {
        return Err(format!("unexpected registry order: {:?}", ids));
    }

    let root = TempRoot(unique_tmp_root());
    let bundled = root.0.join("bundled");
    let user = root.0.join("user");
    let config = root.0.join("config.json");
    fs::create_dir_all(&bundled).map_err(|e| format!("mkdir bundled: {}", e))?;
    fs::create_dir_all(&user).map_err(|e| format!("mkdir user: {}", e))?;
    fs::write(
        bundled.join("registry.json"),
        include_str!("../models/registry.json"),
    )
    .map_err(|e| format!("write bundled registry: {}", e))?;

    let mut manager = ModelManager::new_with_paths(bundled, user, config)?;
    install_model(&mut manager, KROKO_ID)?;

    let wav_path = manager
        .get_model(KROKO_ID)
        .ok_or("kroko install missing")?
        .dir
        .join("test_wavs")
        .join("0.wav");
    if !wav_path.exists() {
        return Err(format!(
            "official Kroko archive did not install a reproducible English WAV at {}",
            wav_path.display()
        ));
    }
    let (samples, sample_rate) = read_wav_mono_f32(&wav_path)?;
    if samples.is_empty() {
        return Err("English WAV contained no samples".into());
    }
    eprintln!(
        "[english-e2e] wav={} samples={} sample_rate={}",
        wav_path.display(),
        samples.len(),
        sample_rate
    );

    install_model(&mut manager, NEMOTRON_ID)?;
    install_model(&mut manager, PARAKEET_ID)?;

    for model_id in [KROKO_ID, NEMOTRON_ID, PARAKEET_ID] {
        let installed = manager
            .get_model(model_id)
            .ok_or_else(|| format!("missing install {}", model_id))?
            .clone();
        let started = Instant::now();
        let recognizer = OnlineRecognizer::new(&installed.dir)
            .map_err(|e| format!("OnlineRecognizer::new failed for {}: {}", model_id, e))?;
        let cold_load = started.elapsed();
        let transcript = transcribe(&Arc::new(recognizer), &samples, sample_rate);
        eprintln!(
            "[english-e2e] model={} cold_load_ms={} transcript={}",
            model_id,
            cold_load.as_millis(),
            transcript
        );
        let visible = transcript
            .trim()
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        if visible == 0 {
            return Err(format!("model {} produced an empty transcript", model_id));
        }
    }
    Ok(())
}

#[test]
fn test_english_models_e2e() {
    if !e2e_enabled() {
        eprintln!(
            "[english-e2e] ECHOLET_ENGLISH_MODEL_E2E is not set; skipping real-network verification (CI-safe)."
        );
        return;
    }
    let result = run_e2e();
    if let Err(err) = &result {
        eprintln!("[english-e2e] FAILED: {}", err);
    }
    assert!(
        result.is_ok(),
        "English model E2E failed: {:?}",
        result.err()
    );
}
