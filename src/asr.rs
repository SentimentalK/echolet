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
        if let Some(ref mt) = manifest.model_type {
            println!("  ModelType: {}", mt);
        }

        let c_encoder = CString::new(encoder.to_str().ok_or("Invalid encoder path")?).unwrap();
        let c_decoder = CString::new(decoder.to_str().ok_or("Invalid decoder path")?).unwrap();
        let c_joiner = CString::new(joiner.to_str().ok_or("Invalid joiner path")?).unwrap();
        let c_tokens = CString::new(tokens.to_str().ok_or("Invalid tokens path")?).unwrap();
        let c_provider = CString::new(manifest.provider.as_str())
            .unwrap_or_else(|_| CString::new("cpu").unwrap());
        let c_decoding = CString::new(manifest.decoding_method.as_str())
            .unwrap_or_else(|_| CString::new("greedy_search").unwrap());
        let c_model_type = manifest
            .model_type
            .as_deref()
            .and_then(|s| CString::new(s).ok());

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
        if let Some(ref mt) = c_model_type {
            config.model_config.model_type = mt.as_ptr();
        }
        config.decoding_method = c_decoding.as_ptr();
        config.max_active_paths = manifest.max_active_paths;
        config.enable_endpoint = 1;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 1.2;
        config.rule3_min_utterance_length = 300.0;

        let raw = unsafe { SherpaOnnxCreateOnlineRecognizer(&config) };
        if raw.is_null() {
            return Err(format!(
                "SherpaOnnxCreateOnlineRecognizer failed for model '{}'",
                manifest.id
            ));
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

    /// Sets a generic per-stream runtime option (Sherpa `OnlineStreamSetOption`).
    ///
    /// `value == None` maps to the empty string, which is the documented
    /// "unset / model default" value for the options Echolet uses (in particular
    /// the multilingual NeMo `"language"` option, where empty means
    /// auto-detect). The Rust wrapper rejects embedded NUL bytes up front so the
    /// C API never receives a truncated key/value.
    pub fn set_option(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        let c_key = checked_cstring("option key", key)?;
        let value_owned = checked_cstring("option value", value.unwrap_or(""))
            .map_err(|e| format!("{} (key {:?})", e, key))?;
        // `value_owned` stays alive for the whole call; the C API copies the
        // string synchronously into the stream's option map.
        unsafe {
            SherpaOnnxOnlineStreamSetOption(self.raw, c_key.as_ptr(), value_owned.as_ptr());
        }
        Ok(())
    }

    /// Forces the per-stream language for multilingual NeMo transducers via the
    /// `"language"` option.
    ///
    /// * `None` or `Some("")` selects auto-detect.
    /// * `Some(code)` (for example `"ja"`) forces that language.
    ///
    /// Must be called after stream creation and before any waveform is fed.
    /// X-ASR (and other single-language models) simply ignore this option, so
    /// calling it is optional and never required for X-ASR inference.
    pub fn set_language(&self, code: Option<&str>) -> Result<(), String> {
        match code {
            None => self.set_option("language", None),
            Some(c) => self.set_option("language", Some(c)),
        }
    }
}

/// Builds a NUL-terminated C string for the Sherpa option API, rejecting any
/// embedded interior NUL so the C layer can never receive a truncated value.
fn checked_cstring(label: &str, value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| format!("{} contains an embedded NUL byte: {:?}", label, value))
}

#[cfg(test)]
mod tests {
    use super::checked_cstring;

    #[test]
    fn checked_cstring_accepts_plain_language_codes() {
        let c = checked_cstring("option value", "ja").expect("plain code must be accepted");
        assert_eq!(c.to_bytes(), b"ja");
        let auto = checked_cstring("option value", "").expect("empty means auto");
        assert_eq!(c_str_bytes(&auto), b"");
    }

    #[test]
    fn checked_cstring_rejects_embedded_nul() {
        let err = checked_cstring("option value", "ja\0evil").expect_err("interior NUL rejected");
        assert!(err.contains("embedded NUL"), "unexpected error: {}", err);
        let key_err = checked_cstring("option key", "lang\0").expect_err("interior NUL rejected");
        assert!(
            key_err.contains("option key"),
            "unexpected error: {}",
            key_err
        );
    }

    fn c_str_bytes(c: &std::ffi::CString) -> &[u8] {
        c.as_c_str().to_bytes()
    }
}
