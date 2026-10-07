use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;

pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

pub trait AudioSource: 'static {
    // Dropping this instance stops and releases the underlying audio capture stream
}

pub type AudioStarter =
    Box<dyn Fn(Sender<AudioChunk>) -> Result<Box<dyn AudioSource>, String>>;

pub struct AudioInput {
    _stream: cpal::Stream,
    pub sample_rate: u32,
}

impl AudioSource for AudioInput {}
impl AudioSource for () {}

impl AudioInput {
    pub fn start(tx: Sender<AudioChunk>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No default input audio device found".to_string())?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        let default_config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels() as usize;
        let sample_format = default_config.sample_format();

        println!("[Audio] ================= Audio Device Info =================");
        println!("[Audio] Device Name:   {}", device_name);
        println!("[Audio] Sample Rate:   {} Hz", sample_rate);
        println!("[Audio] Channel Count: {}", channels);
        println!("[Audio] Sample Format: {:?}", sample_format);
        if channels > 1 {
            println!("[Audio] Multi-channel input detected: Using primary channel (Channel 0) aligned with official sherpa microphone.");
        }
        println!("[Audio] =====================================================");

        let err_fn = |err| eprintln!("[Audio] Stream error: {}", err);

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device
                .build_input_stream(
                    &default_config.into(),
                    move |data: &[f32], _: &_| {
                        let mono = extract_primary_channel(data, channels);
                        let _ = tx.send(AudioChunk {
                            samples: mono,
                            sample_rate,
                        });
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build f32 input stream: {}", e))?,
            cpal::SampleFormat::I16 => device
                .build_input_stream(
                    &default_config.into(),
                    move |data: &[i16], _: &_| {
                        let f32_data: Vec<f32> = data
                            .iter()
                            .map(|&s| s as f32 / 32768.0)
                            .collect();
                        let mono = extract_primary_channel(&f32_data, channels);
                        let _ = tx.send(AudioChunk {
                            samples: mono,
                            sample_rate,
                        });
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build i16 input stream: {}", e))?,
            cpal::SampleFormat::U16 => device
                .build_input_stream(
                    &default_config.into(),
                    move |data: &[u16], _: &_| {
                        let f32_data: Vec<f32> = data
                            .iter()
                            .map(|&s| (s as f32 - 32768.0) / 32768.0)
                            .collect();
                        let mono = extract_primary_channel(&f32_data, channels);
                        let _ = tx.send(AudioChunk {
                            samples: mono,
                            sample_rate,
                        });
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build u16 input stream: {}", e))?,
            format => {
                return Err(format!("Unsupported sample format: {:?}", format));
            }
        };

        stream
            .play()
            .map_err(|e| format!("Failed to start audio stream: {}", e))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
        })
    }
}

/// Extract Channel 0 (primary channel) instead of averaging, exactly matching sherpa-onnx ALSA/PortAudio
fn extract_primary_channel(samples: &[f32], channels: usize) -> Vec<f32> {
    if samples.is_empty() || channels == 0 {
        return Vec::new();
    }
    if channels == 1 {
        samples.to_vec()
    } else {
        samples
            .chunks_exact(channels)
            .map(|chunk| chunk[0])
            .collect()
    }
}
