use std::path::Path;
use std::sync::Arc;
use echolet::asr;

#[test]
fn test_streaming_decode_wav() {
    let model_dir = "/home/sentimentalk/sherpa-onnx/sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16";
    let wav_path = format!("{}/test_wavs/0.wav", model_dir);

    assert!(Path::new(model_dir).exists());
    assert!(Path::new(&wav_path).exists());

    let recognizer = Arc::new(asr::OnlineRecognizer::new(model_dir).expect("Failed to create recognizer"));
    let stream = recognizer.create_stream().expect("Failed to create stream");

    // Read 16-bit PCM wav file (skip 44 bytes header)
    let bytes = std::fs::read(&wav_path).expect("Failed to read wav");
    let pcm_bytes = &bytes[44..];
    let samples: Vec<f32> = pcm_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();

    let chunk_size = 3200; // 0.2s chunks
    let mut intermediate_results = Vec::new();

    for chunk in samples.chunks(chunk_size) {
        stream.accept_waveform(16000, chunk);
        stream.decode_all_ready();
        let r = stream.get_result();
        if !r.is_empty() && (intermediate_results.is_empty() || intermediate_results.last() != Some(&r)) {
            intermediate_results.push(r.clone());
        }
    }

    // Add tail silence
    let tail_silence = vec![0.0f32; 4800];
    stream.accept_waveform(16000, &tail_silence);
    stream.decode_all_ready();

    let final_result = stream.get_result();
    println!("Intermediates: {:?}", intermediate_results);
    println!("Final ASR Result: {}", final_result);

    assert!(!final_result.is_empty(), "ASR result should not be empty");
}
