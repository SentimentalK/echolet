use std::os::raw::{c_char, c_float, c_int};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineTransducerModelConfig {
    pub encoder: *const c_char,
    pub decoder: *const c_char,
    pub joiner: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineParaformerModelConfig {
    pub encoder: *const c_char,
    pub decoder: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineZipformer2CtcModelConfig {
    pub model: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineNemoCtcModelConfig {
    pub model: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineToneCtcModelConfig {
    pub model: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineModelConfig {
    pub transducer: SherpaOnnxOnlineTransducerModelConfig,
    pub paraformer: SherpaOnnxOnlineParaformerModelConfig,
    pub zipformer2_ctc: SherpaOnnxOnlineZipformer2CtcModelConfig,
    pub tokens: *const c_char,
    pub num_threads: i32,
    pub provider: *const c_char,
    pub debug: i32,
    pub model_type: *const c_char,
    pub modeling_unit: *const c_char,
    pub bpe_vocab: *const c_char,
    pub tokens_buf: *const c_char,
    pub tokens_buf_size: i32,
    pub nemo_ctc: SherpaOnnxOnlineNemoCtcModelConfig,
    pub t_one_ctc: SherpaOnnxOnlineToneCtcModelConfig,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxFeatureConfig {
    pub sample_rate: i32,
    pub feature_dim: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineCtcFstDecoderConfig {
    pub graph: *const c_char,
    pub max_active: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxHomophoneReplacerConfig {
    pub dict_dir: *const c_char,
    pub lexicon: *const c_char,
    pub rule_fsts: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SherpaOnnxOnlineRecognizerConfig {
    pub feat_config: SherpaOnnxFeatureConfig,
    pub model_config: SherpaOnnxOnlineModelConfig,
    pub decoding_method: *const c_char,
    pub max_active_paths: i32,
    pub enable_endpoint: i32,
    pub rule1_min_trailing_silence: c_float,
    pub rule2_min_trailing_silence: c_float,
    pub rule3_min_utterance_length: c_float,
    pub hotwords_file: *const c_char,
    pub hotwords_score: c_float,
    pub ctc_fst_decoder_config: SherpaOnnxOnlineCtcFstDecoderConfig,
    pub rule_fsts: *const c_char,
    pub rule_fars: *const c_char,
    pub blank_penalty: c_float,
    pub hotwords_buf: *const c_char,
    pub hotwords_buf_size: i32,
    pub hr: SherpaOnnxHomophoneReplacerConfig,
}

#[repr(C)]
#[derive(Debug)]
pub struct SherpaOnnxOnlineRecognizerResult {
    pub text: *const c_char,
    pub tokens: *const c_char,
    pub tokens_arr: *const *const c_char,
    pub timestamps: *mut c_float,
    pub count: i32,
    pub json: *const c_char,
}

pub enum SherpaOnnxOnlineRecognizer {}
pub enum SherpaOnnxOnlineStream {}

#[link(name = "sherpa-onnx-c-api")]
extern "C" {
    pub fn SherpaOnnxCreateOnlineRecognizer(
        config: *const SherpaOnnxOnlineRecognizerConfig,
    ) -> *const SherpaOnnxOnlineRecognizer;

    pub fn SherpaOnnxDestroyOnlineRecognizer(
        recognizer: *const SherpaOnnxOnlineRecognizer,
    );

    pub fn SherpaOnnxCreateOnlineStream(
        recognizer: *const SherpaOnnxOnlineRecognizer,
    ) -> *const SherpaOnnxOnlineStream;

    pub fn SherpaOnnxDestroyOnlineStream(stream: *const SherpaOnnxOnlineStream);

    pub fn SherpaOnnxOnlineStreamAcceptWaveform(
        stream: *const SherpaOnnxOnlineStream,
        sample_rate: i32,
        samples: *const c_float,
        n: i32,
    );

    pub fn SherpaOnnxIsOnlineStreamReady(
        recognizer: *const SherpaOnnxOnlineRecognizer,
        stream: *const SherpaOnnxOnlineStream,
    ) -> c_int;

    pub fn SherpaOnnxDecodeOnlineStream(
        recognizer: *const SherpaOnnxOnlineRecognizer,
        stream: *const SherpaOnnxOnlineStream,
    );

    pub fn SherpaOnnxGetOnlineStreamResult(
        recognizer: *const SherpaOnnxOnlineRecognizer,
        stream: *const SherpaOnnxOnlineStream,
    ) -> *const SherpaOnnxOnlineRecognizerResult;

    pub fn SherpaOnnxDestroyOnlineRecognizerResult(
        r: *const SherpaOnnxOnlineRecognizerResult,
    );

    pub fn SherpaOnnxOnlineStreamIsEndpoint(
        recognizer: *const SherpaOnnxOnlineRecognizer,
        stream: *const SherpaOnnxOnlineStream,
    ) -> c_int;

    pub fn SherpaOnnxOnlineStreamReset(
        recognizer: *const SherpaOnnxOnlineRecognizer,
        stream: *const SherpaOnnxOnlineStream,
    );

    pub fn SherpaOnnxOnlineStreamInputFinished(
        stream: *const SherpaOnnxOnlineStream,
    );
}
