use echolet::asr::OnlineRecognizer;
use echolet::models::manifest::ModelManifest;
use std::path::Path;
use std::sync::Arc;

#[test]
fn test_multi_model_initialization_and_inference() {
    let en_dir = Path::new("/tmp/model-test/en/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17");
    let zh_dir = Path::new("/tmp/model-test/zh/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30");

    // Only run if candidates were extracted in /tmp/
    if !en_dir.exists() || !zh_dir.exists() {
        println!("[Skip] Candidate directories not present in /tmp/model-test");
        return;
    }

    // 1. Test English Model ASR
    let en_manifest = ModelManifest {
        id: "sherpa-onnx-streaming-zipformer-en-20M-2023-02-17".into(),
        display_name: "English (Zipformer-20M)".into(),
        version: "2023-02-17".into(),
        language: "en".into(),
        family: "online-transducer".into(),
        encoder: "encoder-epoch-99-avg-1.int8.onnx".into(),
        decoder: "decoder-epoch-99-avg-1.onnx".into(),
        joiner: "joiner-epoch-99-avg-1.int8.onnx".into(),
        tokens: "tokens.txt".into(),
        ..Default::default()
    };

    let en_rec = Arc::new(OnlineRecognizer::from_manifest(en_dir, &en_manifest).expect("Failed to init English model"));
    let en_stream = en_rec.create_stream().expect("Failed to create English stream");

    let en_wav = en_dir.join("test_wavs/0.wav");
    if en_wav.exists() {
        let bytes = std::fs::read(&en_wav).expect("Failed to read EN wav");
        let samples: Vec<f32> = bytes[44..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();

        en_stream.accept_waveform(16000, &samples);
        let tail = vec![0.0f32; 4800];
        en_stream.accept_waveform(16000, &tail);
        en_stream.decode_all_ready();

        let res = en_stream.get_result();
        println!("[Multi-Model Test] English result: {}", res);
        assert!(!res.is_empty(), "English recognition result should not be empty");
    }

    // 2. Test Chinese Model ASR
    let zh_manifest = ModelManifest {
        id: "sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30".into(),
        display_name: "Chinese (Zipformer-int8)".into(),
        version: "2025-06-30".into(),
        language: "zh".into(),
        family: "online-transducer".into(),
        encoder: "encoder.int8.onnx".into(),
        decoder: "decoder.onnx".into(),
        joiner: "joiner.int8.onnx".into(),
        tokens: "tokens.txt".into(),
        ..Default::default()
    };

    let zh_rec = Arc::new(OnlineRecognizer::from_manifest(zh_dir, &zh_manifest).expect("Failed to init Chinese model"));
    let zh_stream = zh_rec.create_stream().expect("Failed to create Chinese stream");

    let zh_wav = zh_dir.join("test_wavs/0.wav");
    if zh_wav.exists() {
        let bytes = std::fs::read(&zh_wav).expect("Failed to read ZH wav");
        let samples: Vec<f32> = bytes[44..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();

        zh_stream.accept_waveform(16000, &samples);
        let tail = vec![0.0f32; 4800];
        zh_stream.accept_waveform(16000, &tail);
        zh_stream.decode_all_ready();

        let res = zh_stream.get_result();
        println!("[Multi-Model Test] Chinese result: {}", res);
        assert!(!res.is_empty(), "Chinese recognition result should not be empty");
    }
}
