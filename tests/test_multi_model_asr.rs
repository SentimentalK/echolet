use echolet::asr::OnlineRecognizer;
use echolet::models::manifest::ModelManifest;
use std::path::Path;
use std::sync::Arc;

#[test]
fn test_multi_model_initialization_and_inference() {
    let nemotron_dir = Path::new(
        "/home/sentimentalk/.cache/echolet-staging/nemotron/sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25",
    );
    let xasr_dir = Path::new("/home/sentimentalk/.cache/echolet-staging/xasr");
    let fallback_wav = Path::new("/home/sentimentalk/codes/echolet/.local-runtime/models/bilingual-zh-en/test_wavs/0.wav");

    // 1. Test Nemotron 2026 Model ASR if staged
    if nemotron_dir.exists() {
        let nemo_manifest = ModelManifest {
            id: "sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25".into(),
            display_name: "English (Nemotron 0.6B / 560ms)".into(),
            version: "2026-04-25".into(),
            language: "en".into(),
            family: "online-transducer".into(),
            encoder: "encoder.int8.onnx".into(),
            decoder: "decoder.int8.onnx".into(),
            joiner: "joiner.int8.onnx".into(),
            tokens: "tokens.txt".into(),
            ..Default::default()
        };

        let nemo_rec = Arc::new(
            OnlineRecognizer::from_manifest(nemotron_dir, &nemo_manifest)
                .expect("Failed to init Nemotron model"),
        );
        let nemo_stream = nemo_rec
            .create_stream()
            .expect("Failed to create Nemotron stream");

        let wav_path = nemotron_dir.join("test_wavs/0.wav");
        let target_wav = if wav_path.exists() {
            &wav_path
        } else {
            fallback_wav
        };

        if target_wav.exists() {
            let bytes = std::fs::read(target_wav).expect("Failed to read EN wav");
            let samples: Vec<f32> = bytes[44..]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();

            nemo_stream.accept_waveform(16000, &samples);
            let tail = vec![0.0f32; 4800];
            nemo_stream.accept_waveform(16000, &tail);
            nemo_stream.decode_all_ready();

            let res = nemo_stream.get_result();
            println!("[Multi-Model Test] Nemotron 2026 result: {}", res);
            assert!(
                !res.is_empty(),
                "Nemotron recognition result should not be empty"
            );
        }
    }

    // 2. Test X-ASR 2026 Model ASR if staged
    if xasr_dir.exists() {
        let xasr_manifest = ModelManifest {
            id: "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1".into(),
            display_name: "Chinese + English (X-ASR / 480ms)".into(),
            version: "2026".into(),
            language: "zh-en".into(),
            family: "online-transducer".into(),
            encoder: "encoder-480ms.onnx".into(),
            decoder: "decoder-480ms.onnx".into(),
            joiner: "joiner-480ms.onnx".into(),
            tokens: "tokens.txt".into(),
            model_type: Some("zipformer2".into()),
            ..Default::default()
        };

        let xasr_rec = Arc::new(
            OnlineRecognizer::from_manifest(xasr_dir, &xasr_manifest)
                .expect("Failed to init X-ASR model"),
        );
        let xasr_stream = xasr_rec
            .create_stream()
            .expect("Failed to create X-ASR stream");

        if fallback_wav.exists() {
            let bytes = std::fs::read(fallback_wav).expect("Failed to read bilingual wav");
            let samples: Vec<f32> = bytes[44..]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();

            xasr_stream.accept_waveform(16000, &samples);
            let tail = vec![0.0f32; 4800];
            xasr_stream.accept_waveform(16000, &tail);
            xasr_stream.decode_all_ready();

            let res = xasr_stream.get_result();
            println!("[Multi-Model Test] X-ASR 2026 result: {}", res);
            assert!(!res.is_empty(), "X-ASR recognition result should not be empty");
        }
    }
}
