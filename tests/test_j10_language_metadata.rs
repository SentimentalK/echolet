//! PROJECT-041 Stage 3 / J10 — deterministic language-tier / selection tests.
//!
//! These tests are fully offline and run in ordinary CI. They pin the accepted
//! J9 taxonomy (19 transcription-ready + 13 broad-coverage + 8 adaptation-ready
//! = 40 locales; 32 out-of-box), prove BCP-47 locales are never run through the
//! legacy hyphen-splitting normalizer, and prove the selection contract rejects
//! adaptation-only locales.

use echolet::models::language::normalize_language_spec;
use echolet::models::manifest::ModelManifest;
use echolet::models::registry::{
    LanguageTier, ModelLanguageOption, ModelLanguageOptions, ModelRegistry, VerificationStatus,
};
use std::collections::BTreeSet;

const NEMOTRON_ID: &str = "echolet-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11-r1";
const XASR_ID: &str = "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1";

fn def_options() -> ModelLanguageOptions {
    #[derive(serde::Deserialize)]
    struct Def {
        language_options: ModelLanguageOptions,
    }
    let def: Def = serde_json::from_str(include_str!("../models/nemotron-model.json"))
        .expect("models/nemotron-model.json must parse");
    def.language_options
}

fn registry() -> ModelRegistry {
    ModelRegistry::from_str(include_str!("../models/registry.json"))
        .expect("shipped registry must parse")
}

#[test]
fn test_tier_counts_and_membership() {
    let opts = def_options();
    assert_eq!(opts.transcription_ready_count(), 19);
    assert_eq!(opts.broad_coverage_count(), 13);
    assert_eq!(opts.adaptation_ready_count(), 8);
    assert_eq!(
        opts.supported.len(),
        32,
        "out-of-box selectable options must be exactly 19 + 13"
    );

    // Japanese is transcription-ready; Mandarin zh-CN is broad-coverage.
    let ja = opts.find_supported("ja-JP").expect("ja-JP supported");
    assert_eq!(ja.tier, LanguageTier::TranscriptionReady);
    let zh = opts.find_supported("zh-CN").expect("zh-CN supported");
    assert_eq!(zh.tier, LanguageTier::BroadCoverage);

    // No adaptation-ready locale leaked into the selectable set.
    for adaptation in &opts.adaptation_ready {
        assert!(
            opts.find_supported(&adaptation.locale).is_none(),
            "adaptation-ready locale {} must not be selectable",
            adaptation.locale
        );
    }
}

#[test]
fn test_runtime_code_mapping_and_bcp47_lookup() {
    let opts = def_options();
    // BCP-47 locale lookup does NOT split on '-': en-GB resolves to runtime "en".
    let en_gb = opts.find_supported("en-GB").expect("en-GB");
    assert_eq!(en_gb.runtime_code, "en");
    // runtime-code lookup works too.
    assert_eq!(
        opts.find_supported("ja").map(|o| o.locale.as_str()),
        Some("ja-JP")
    );
    // Case-insensitive locale lookup.
    assert!(opts.find_supported("JA-jp").is_some());

    // Document the hazard: the legacy normalizer would split "ja-JP".
    assert_eq!(normalize_language_spec("ja-JP"), vec!["ja", "jp"]);
    // ...but the typed lookup keeps it intact.
    assert_eq!(opts.find_supported("ja-JP").unwrap().locale, "ja-JP");
}

#[test]
fn test_all_supported_locales_are_bcp47_and_distinct() {
    let opts = def_options();
    let locales: BTreeSet<&str> = opts.supported.iter().map(|o| o.locale.as_str()).collect();
    assert_eq!(locales.len(), 32, "all supported locales distinct");
    for o in &opts.supported {
        assert!(
            o.locale.contains('-'),
            "expected BCP-47 locale, got {:?}",
            o.locale
        );
        assert!(!o.runtime_code.is_empty());
        assert!(o.tier.is_selectable());
        assert_ne!(o.tier, LanguageTier::AdaptationReady);
    }
}

#[test]
fn test_language_options_round_trip_is_lossless() {
    let opts = def_options();
    let json = serde_json::to_string(&opts).expect("serialize options");
    let back: ModelLanguageOptions = serde_json::from_str(&json).expect("deserialize options");
    assert_eq!(opts, back, "language metadata round-trip must be lossless");
}

fn historical_multilingual_entry() -> echolet::models::registry::RegistryModelEntry {
    echolet::models::registry::RegistryModelEntry {
        id: NEMOTRON_ID.to_string(),
        display_name: "Multilingual Nemotron 3.5 ASR Streaming 0.6B / 560ms (int8)".to_string(),
        version: "2026-06-11-r1".to_string(),
        languages: vec![
            "en".into(),
            "es".into(),
            "fr".into(),
            "it".into(),
            "pt".into(),
            "nl".into(),
            "de".into(),
            "tr".into(),
            "ru".into(),
            "ar".into(),
            "hi".into(),
            "ja".into(),
            "ko".into(),
            "vi".into(),
            "uk".into(),
            "pl".into(),
            "sv".into(),
            "cs".into(),
            "nb".into(),
            "da".into(),
            "bg".into(),
            "fi".into(),
            "hr".into(),
            "sk".into(),
            "zh".into(),
            "hu".into(),
            "ro".into(),
            "et".into(),
        ],
        family: "online-transducer".to_string(),
        source: echolet::models::registry::ModelSource {
            bundled: false,
            url: None,
            sha256: None,
            repository: Some(
                "https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b".into(),
            ),
            revision: Some("nvidia/nemotron-3.5-asr-streaming-0.6b".into()),
        },
        files: echolet::models::registry::ModelFilesConfig {
            encoder: "encoder.int8.onnx".into(),
            decoder: "decoder.int8.onnx".into(),
            joiner: "joiner.int8.onnx".into(),
            tokens: "tokens.txt".into(),
        },
        runtime: echolet::models::registry::ModelRuntimeConfig {
            model_type: None,
            sample_rate: 16000,
            feature_dim: 80,
            num_threads: 1,
            provider: "cpu".into(),
            decoding_method: "greedy_search".into(),
            max_active_paths: 4,
        },
        download_size_bytes: Some(456709352),
        installed_size_bytes: Some(682215356),
        upstream_release_date: Some("2026-06-11".into()),
        license: None,
        language_options: Some(def_options()),
        verification_status: VerificationStatus::EcholetVerified,
    }
}

#[test]
fn test_registry_does_not_contain_multilingual_nemotron_and_authorities_agree() {
    let reg = registry();
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.lock.json")).unwrap();
    let def: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.json")).unwrap();

    // The current product catalog must NOT contain the historical multilingual Nemotron model.
    assert!(
        reg.get_model(NEMOTRON_ID).is_none(),
        "Old multilingual Nemotron model must be removed from current product registry"
    );

    // Historical definition and lock authorities agree with each other.
    assert_eq!(def["id"].as_str(), lock["id"].as_str());
    assert_eq!(
        def["language_options"]["supported"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
}

#[test]
fn test_selection_contract_rejects_adaptation_and_unknown() {
    let entry = historical_multilingual_entry();

    // Auto is valid and resolves to no forced option.
    assert_eq!(entry.validate_language_selection(None), Ok(None));
    assert_eq!(entry.validate_language_selection(Some("")), Ok(None));
    assert_eq!(entry.validate_language_selection(Some("auto")), Ok(None));

    // Supported locale resolves.
    let ja = entry
        .validate_language_selection(Some("ja-JP"))
        .unwrap()
        .expect("ja-JP resolves to an option");
    assert_eq!(ja.runtime_code, "ja");

    // Adaptation-ready locale is explicitly rejected.
    for adaptation in entry.adaptation_ready_locales() {
        let err = entry
            .validate_language_selection(Some(&adaptation.locale))
            .expect_err("adaptation-ready must be rejected");
        assert!(
            err.contains("adaptation-ready"),
            "unexpected error: {}",
            err
        );
    }

    // Unknown locale rejected.
    let err = entry
        .validate_language_selection(Some("xx-XX"))
        .expect_err("unknown must be rejected");
    assert!(err.contains("not a supported"), "unexpected error: {}", err);
}

#[test]
fn test_supported_language_options_never_returns_adaptation() {
    let entry = historical_multilingual_entry();
    let supported = entry.supported_language_options();
    assert_eq!(supported.len(), 32);
    assert!(supported.iter().all(|o| o.tier.is_selectable()));
    for a in entry.adaptation_ready_locales() {
        assert!(
            !supported.iter().any(|o| o.locale == a.locale),
            "adaptation locale {} leaked into supported API",
            a.locale
        );
    }
}

#[test]
fn test_x_asr_remains_default_and_simple() {
    let reg = registry();
    assert_eq!(
        reg.default_model_id, XASR_ID,
        "X-ASR must remain the default model"
    );
    let xasr = reg.get_model(XASR_ID).unwrap();
    assert_eq!(
        xasr.verification_status,
        VerificationStatus::EcholetVerified
    );
    // X-ASR has no typed locale metadata and rejects any forced language.
    assert!(xasr.language_options.is_none());
    assert!(xasr.supported_language_options().is_empty());
    assert_eq!(xasr.validate_language_selection(None), Ok(None));
    assert!(xasr.validate_language_selection(Some("ja")).is_err());
    // Coarse base languages are unchanged.
    assert_eq!(xasr.languages, vec!["zh", "en"]);
}

#[test]
fn test_manifest_propagates_language_options() {
    let entry = historical_multilingual_entry();
    let manifest: ModelManifest = entry.to_manifest();
    assert_eq!(manifest.supported_language_options().len(), 32);
    assert_eq!(
        manifest
            .validate_language_selection(Some("ja-JP"))
            .unwrap()
            .expect("ja-JP resolves")
            .runtime_code,
        "ja"
    );
    assert!(manifest.validate_language_selection(Some("th-TH")).is_err());
    // Round-trip through serde preserves the typed metadata.
    let json = serde_json::to_string(&manifest).unwrap();
    let back: ModelManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(manifest, back);
}

#[test]
fn test_legacy_normalizer_still_handles_base_languages() {
    // The Nemotron coarse base-language list is intentionally free of BCP-47
    // values, so the legacy normalizer cannot corrupt it on reparse.
    let opts = def_options();
    let entry = historical_multilingual_entry();
    for lang in &entry.languages {
        assert!(
            !lang.contains('-'),
            "coarse languages must not contain BCP-47 locales: {}",
            lang
        );
    }
    // zh-CN and ja-JP are represented as coarse base codes in `languages`.
    assert!(entry.languages.iter().any(|l| l == "zh"));
    assert!(entry.languages.iter().any(|l| l == "ja"));
    // Round-tripping the registry does not alter the typed options.
    let reg = ModelRegistry {
        schema_version: 2,
        default_model_id: entry.id.clone(),
        models: vec![entry],
    };
    let serialized = reg.to_canonical_string().unwrap();
    let reparsed = ModelRegistry::from_str(&serialized).unwrap();
    assert_eq!(
        reparsed.get_model(NEMOTRON_ID).unwrap().language_options,
        Some(opts)
    );
}

#[test]
fn test_english_models_have_no_language_options() {
    let reg = registry();
    let zipformer = reg
        .get_model("echolet-zipformer-streaming-en-2023-06-26-r1")
        .expect("Zipformer entry");
    assert_eq!(zipformer.languages, vec!["en"]);
    assert!(zipformer.language_options.is_none());
    assert!(zipformer.supported_language_options().is_empty());
    assert_eq!(zipformer.validate_language_selection(None), Ok(None));
    assert!(zipformer.validate_language_selection(Some("en")).is_err());

    let nemotron = reg
        .get_model("echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1")
        .expect("Nemotron English entry");
    assert_eq!(nemotron.languages, vec!["en"]);
    assert!(nemotron.language_options.is_none());
    assert!(nemotron.supported_language_options().is_empty());
    assert_eq!(nemotron.validate_language_selection(None), Ok(None));
    assert!(nemotron.validate_language_selection(Some("en")).is_err());
}

#[test]
fn test_frozen_lock_packaging_invariants() {
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.lock.json")).unwrap();
    let canonical: Vec<&str> = lock["canonical_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    // A future repack must not be able to silently drop the license/origin
    // notices: they are part of the required canonical file set.
    for required in ["LICENSE", "NOTICE", "UPSTREAM.json"] {
        assert!(
            canonical.contains(&required),
            "canonical_files must require {required}"
        );
    }
    assert_eq!(
        lock["required_license_files"].as_array().unwrap().len(),
        2,
        "LICENSE and NOTICE are the mandatory redistribution artifacts"
    );
    // No test audio may ever be part of the frozen product payload.
    assert!(
        canonical.iter().all(|f| !f.ends_with(".wav")),
        "frozen pack must not contain test WAVs"
    );
    assert_eq!(
        lock["license"]["spdx"].as_str(),
        Some("OpenMDW-1.1"),
        "frozen pack license identity"
    );

    // The four model files are pinned individually.
    for f in [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ] {
        assert!(
            lock["file_sha256"][f].as_str().is_some(),
            "missing pinned SHA for {f}"
        );
    }

    // The definition authority and the release lock agree on identity.
    let def: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.json")).unwrap();
    assert_eq!(def["id"].as_str(), lock["id"].as_str());
    assert_eq!(
        def["upstream_source"]["sha256"].as_str(),
        lock["upstream_source"]["sha256"].as_str()
    );
}

#[test]
fn test_language_option_model_round_trip() {
    let opt = ModelLanguageOption {
        locale: "ja-JP".into(),
        runtime_code: "ja".into(),
        tier: LanguageTier::TranscriptionReady,
        display_name: Some("Japanese (Japan)".into()),
    };
    let json = serde_json::to_string(&opt).unwrap();
    assert!(json.contains("TranscriptionReady"));
    let back: ModelLanguageOption = serde_json::from_str(&json).unwrap();
    assert_eq!(opt, back);
}
