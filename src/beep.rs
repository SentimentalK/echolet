use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub fn beep_start() {
    if std::env::var_os("ECHOLET_DISABLE_BEEP").is_some() {
        return;
    }
    thread::spawn(|| {
        play_tone(1000.0, 70, 0.25); // High pitch beep (1000Hz, 70ms)
    });
}

pub fn beep_stop() {
    if std::env::var_os("ECHOLET_DISABLE_BEEP").is_some() {
        return;
    }
    thread::spawn(|| {
        play_tone(450.0, 80, 0.25); // Low pitch beep (450Hz, 80ms)
    });
}

fn play_tone(freq: f32, duration_ms: u64, volume: f32) {
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => {
            eprintln!("[Beep] No default output device found; tone not played.");
            return;
        }
    };

    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
    let config = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[Beep] Failed to get output config for '{}': {}", device_name, e);
            return;
        }
    };

    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    if channels == 0 || sample_rate == 0 {
        eprintln!("[Beep] Invalid output config: rate={}, channels={}", sample_rate, channels);
        return;
    }
    println!(
        "[Beep] Playing tone: freq={}Hz, {}ms, vol={} -> device='{}', rate={}Hz, channels={}, fmt={:?}",
        freq, duration_ms, volume, device_name, sample_rate, channels, sample_format
    );

    let total_frames = (sample_rate as u64 * duration_ms / 1000) as usize;
    let attack_frames = (sample_rate as usize * 5 / 1000).max(1);
    let release_frames = (sample_rate as usize * 10 / 1000).max(1);

    let mut samples = Vec::with_capacity(total_frames);
    for i in 0..total_frames {
        let t = i as f32 / sample_rate as f32;
        let sine = (t * freq * 2.0 * std::f32::consts::PI).sin();

        let env = if i < attack_frames {
            i as f32 / attack_frames as f32
        } else if i + release_frames > total_frames {
            (total_frames - i) as f32 / release_frames as f32
        } else {
            1.0
        };

        samples.push(sine * env * volume);
    }

    let samples = Arc::new(samples);
    let sample_idx = Arc::new(AtomicUsize::new(0));
    let is_done = Arc::new(AtomicBool::new(false));

    let samples_clone = samples.clone();
    let idx_clone = sample_idx.clone();
    let done_clone = is_done.clone();

    let err_fn = |_| {};

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.into(),
            move |output: &mut [f32], _: &_| {
                for frame in output.chunks_mut(channels) {
                    let idx = idx_clone.fetch_add(1, Ordering::Relaxed);
                    let val = if idx < samples_clone.len() {
                        samples_clone[idx]
                    } else {
                        done_clone.store(true, Ordering::Relaxed);
                        0.0
                    };
                    for sample in frame.iter_mut() {
                        *sample = val;
                    }
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config.into(),
            move |output: &mut [i16], _: &_| {
                for frame in output.chunks_mut(channels) {
                    let idx = idx_clone.fetch_add(1, Ordering::Relaxed);
                    let val = if idx < samples_clone.len() {
                        (samples_clone[idx] * 32767.0) as i16
                    } else {
                        done_clone.store(true, Ordering::Relaxed);
                        0
                    };
                    for sample in frame.iter_mut() {
                        *sample = val;
                    }
                }
            },
            err_fn,
            None,
        ),
        _ => return,
    };

    if let Ok(stream) = stream {
        match stream.play() {
            Ok(()) => {
                // Hold the stream alive well past the tone so it can actually pass through
                // WASAPI shared-mode output latency (~50-200ms), otherwise a 70-80ms beep
                // gets dropped before reaching the speaker.
                thread::sleep(Duration::from_millis(duration_ms + 300));
                println!("[Beep] Tone playback finished.");
            }
            Err(e) => {
                eprintln!("[Beep] stream.play() failed on '{}': {}", device_name, e);
            }
        }
    } else {
        eprintln!("[Beep] Failed to build output stream on '{}' (fmt={:?}).", device_name, sample_format);
    }
}
