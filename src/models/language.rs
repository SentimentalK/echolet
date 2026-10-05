//! Centralized language normalization shared by the model registry and the
//! on-disk model manifest.
//!
//! The product historically stored a single legacy `language` string such as
//! `"zh-en"`. The v2 registry schema and the manifest now store a canonical
//! plural `languages` list (for example `["zh", "en"]`). All legacy-to-plural
//! normalization flows through this module so there is exactly one authority
//! for language semantics.

use serde::de::Deserializer;
use serde::Deserialize;
use std::collections::HashSet;

/// Splits a legacy language spec such as `"zh-en"` or `"en"` into canonical
/// lowercase language codes.
///
/// Recognized separators are `-`, `_`, `+`, `/`, `,` and whitespace. Empty
/// fragments are discarded.
pub fn normalize_language_spec(spec: &str) -> Vec<String> {
    spec.split(|c: char| {
        c == '-' || c == '_' || c == '+' || c == '/' || c == ',' || c.is_whitespace()
    })
    .map(|part| part.trim().to_ascii_lowercase())
    .filter(|part| !part.is_empty())
    .collect()
}

/// Normalizes any mix of legacy singular and plural language specs into an
/// order-preserving, de-duplicated list of lowercase codes.
pub fn normalize_languages<I, S>(specs: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for spec in specs {
        for code in normalize_language_spec(spec.as_ref()) {
            if seen.insert(code.clone()) {
                out.push(code);
            }
        }
    }
    out
}

/// Stable, canonical key for a normalized language set.
///
/// This is an intentional, single-source-of-truth serialization of the
/// normalized list (for example `["zh", "en"] -> "zh-en"`), not the brittle
/// ad-hoc string joining the v1 product code relied on.
pub fn language_key(languages: &[String]) -> String {
    languages.join("-")
}

/// Human-readable label for a single language code. Unknown codes fall back to
/// the code itself so no model becomes unusable because of an unrecognized
/// language.
pub fn language_code_label(code: &str) -> &str {
    match code {
        "zh" => "Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "ru" => "Russian",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ar" => "Arabic",
        "hi" => "Hindi",
        other => other,
    }
}

/// Human-readable label for a language set (for example `"Chinese + English"`).
pub fn language_label(languages: &[String]) -> String {
    languages
        .iter()
        .map(|code| language_code_label(code).to_string())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Deserializes either a legacy singular `language` string or a plural
/// `languages` array, always producing a normalized plural list.
///
/// This is the single compatibility seam that lets legacy v1 registry files and
/// legacy on-disk manifests load without a second in-memory `language` field.
pub fn deserialize_languages<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum LanguageSpec {
        Single(String),
        Many(Vec<String>),
    }

    match LanguageSpec::deserialize(deserializer)? {
        LanguageSpec::Single(spec) => Ok(normalize_language_spec(&spec)),
        LanguageSpec::Many(specs) => Ok(normalize_languages(specs)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_legacy_bilingual_spec() {
        assert_eq!(normalize_language_spec("zh-en"), vec!["zh", "en"]);
        assert_eq!(normalize_language_spec("en"), vec!["en"]);
        assert_eq!(normalize_language_spec("ZH_EN"), vec!["zh", "en"]);
        assert_eq!(normalize_language_spec("zh,en"), vec!["zh", "en"]);
        assert!(normalize_language_spec("").is_empty());
    }

    #[test]
    fn normalizes_and_dedupes_mixed_specs() {
        let normalized = normalize_languages(vec![
            "zh-en".to_string(),
            "en".to_string(),
            "ja".to_string(),
        ]);
        assert_eq!(normalized, vec!["zh", "en", "ja"]);
    }

    #[test]
    fn derives_key_and_label() {
        let languages = vec!["zh".to_string(), "en".to_string()];
        assert_eq!(language_key(&languages), "zh-en");
        assert_eq!(language_label(&languages), "Chinese + English");
    }
}
