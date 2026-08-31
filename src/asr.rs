use crate::ffi::*;
use crate::models::manifest::ModelManifest;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::Arc;

pub struct OnlineRecognizer {
    raw: *const SherpaOnnxOnlineRecognizer,
}

// Safety: SherpaOnnxOnlineRecognizer is thread-safe for creating streams and read-only model inference
unsafe impl Send for OnlineRecognizer {}
unsafe impl Sync for OnlineRecognizer {}

impl Drop for OnlineRecognizer {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                SherpaOnnxDestroyOnlineRecognizer(self.raw);
            }
        }
    }
}

pub struct OnlineStream {
    raw: *const SherpaOnnxOnlineStream,
    recognizer: Arc<OnlineRecognizer>,
}

unsafe impl Send for OnlineStream {}

impl Drop for OnlineStream {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                SherpaOnnxDestroyOnlineStream(self.raw);
            }
        }
    }
}

impl OnlineRecognizer {
    pub fn new<P: AsRef<Path>>(model_dir: P) -> Result<Self, String> {
        let model_dir = model_dir.as_ref();
        let manifest_path = model_dir.join("model.json");
        let manifest = if manifest_path.exists() {
            ModelManifest::from_file(&manifest_path)?
        } else {
            ModelManifest::default()
        };
        Self::from_manifest(model_dir, &manifest)
    }

    pub fn from_manifest<P: AsRef<Path>>(
        model_dir: P,
        manifest: &ModelManifest,
    ) -> Result<Self, String> {
        let model_dir = model_dir.as_ref();

        manifest.validate_files(model_dir)?;

        let encoder = model_dir.join(&manifest.encoder);
        let decoder = model_dir.join(&manifest.decoder);
        let joiner = model_dir.join(&manifest.joiner);
        let tokens = model_dir.join(&manifest.tokens);

        println!("[ASR] Initializing Recognizer for model '{}':", manifest.id);
        println!("  Encoder: {:?}", encoder);
        println!("  Decoder: {:?}", decoder);
        println!("  Joiner:  {:?}", joiner);
        println!("  Tokens:  {:?}", tokens);

        let c_encoder = CString::new(encoder.to_str().ok_or("Invalid encoder path")?).unwrap();
        let c_decoder = CString::new(decoder.to_str().ok_or("Invalid decoder path")?).unwrap();
        let c_joiner = CString::new(joiner.to_str().ok_or("Invalid joiner path")?).unwrap();
        let c_tokens = CString::new(tokens.to_str().ok_or("Invalid tokens path")?).unwrap();
        let c_provider = CString::new(manifest.provider.as_str()).unwrap_or_else(|_| CString::new("cpu").unwrap());
        let c_decoding = CString::new(manifest.decoding_method.as_str()).unwrap_or_else(|_| CString::new("greedy_search").unwrap());

        let mut config: SherpaOnnxOnlineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = manifest.sample_rate as i32;
        config.feat_config.feature_dim = manifest.feature_dim;
        config.model_config.transducer.encoder = c_encoder.as_ptr();
        config.model_config.transducer.decoder = c_decoder.as_ptr();
        config.model_config.transducer.joiner = c_joiner.as_ptr();
        config.model_config.tokens = c_tokens.as_ptr();
        config.model_config.num_threads = manifest.num_threads;
        config.model_config.provider = c_provider.as_ptr();
        config.model_config.debug = 0;
        config.decoding_method = c_decoding.as_ptr();
        config.max_active_paths = manifest.max_active_paths;
        config.enable_endpoint = 1;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 1.2;
        config.rule3_min_utterance_length = 300.0;

        let raw = unsafe { SherpaOnnxCreateOnlineRecognizer(&config) };
        if raw.is_null() {
            return Err(format!("SherpaOnnxCreateOnlineRecognizer failed for model '{}'", manifest.id));
        }

        Ok(Self { raw })
    }

    pub fn create_stream(self: &Arc<Self>) -> Result<OnlineStream, String> {
        let stream_raw = unsafe { SherpaOnnxCreateOnlineStream(self.raw) };
        if stream_raw.is_null() {
            return Err("SherpaOnnxCreateOnlineStream returned null".to_string());
        }
        Ok(OnlineStream {
            raw: stream_raw,
            recognizer: Arc::clone(self),
        })
    }
}

impl OnlineStream {
    pub fn accept_waveform(&self, sample_rate: i32, samples: &[f32]) {
        unsafe {
            SherpaOnnxOnlineStreamAcceptWaveform(
                self.raw,
                sample_rate,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }
    }

    pub fn is_ready(&self) -> bool {
        unsafe { SherpaOnnxIsOnlineStreamReady(self.recognizer.raw, self.raw) != 0 }
    }

    pub fn decode(&self) {
        unsafe {
            SherpaOnnxDecodeOnlineStream(self.recognizer.raw, self.raw);
        }
    }

    pub fn decode_all_ready(&self) {
        while self.is_ready() {
            self.decode();
        }
    }

    pub fn get_result(&self) -> String {
        unsafe {
            let res_ptr = SherpaOnnxGetOnlineStreamResult(self.recognizer.raw, self.raw);
            if res_ptr.is_null() {
                return String::new();
            }
            let text = if (*res_ptr).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*res_ptr).text)
                    .to_string_lossy()
                    .into_owned()
            };
            SherpaOnnxDestroyOnlineRecognizerResult(res_ptr);
            text
        }
    }

    pub fn is_endpoint(&self) -> bool {
        unsafe { SherpaOnnxOnlineStreamIsEndpoint(self.recognizer.raw, self.raw) != 0 }
    }

    pub fn reset(&self) {
        unsafe {
            SherpaOnnxOnlineStreamReset(self.recognizer.raw, self.raw);
        }
    }
}
