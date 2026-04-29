//! DeepL HTTP translation client + rolling context buffer.
//!
//! See DESIGN.md §4.6 and realtime_translator_spec.md → "DeepL Text Translation".
//!
//! `TranslationContext` keeps the last N flushed *source* sentences. Each
//! translation call passes them as DeepL's `context` parameter (not billed,
//! improves coherence). The window slides: new sentence in, oldest out.
//!
//! Source language defaults to `None` (DeepL auto-detects). Pass an explicit
//! BCP-47 code to pin it.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Configuration for the DeepL client.
#[derive(Debug, Clone)]
pub struct DeepLConfig {
    pub api_key:     String,
    /// `None` → DeepL auto-detects the source language (default).
    /// `Some("DE")` → pin to a specific language.
    pub source_lang: Option<String>,
    pub target_lang: String,
    pub model_type:  String,
    /// `"https://api-free.deepl.com/v2"` for free keys, `"https://api.deepl.com/v2"` for paid.
    pub endpoint:    String,
    /// How many prior source sentences to keep in the context window.
    pub context_sentences: usize,
}

impl DeepLConfig {
    /// `source_lang = None` → auto-detect. Pass `Some("DE")` to pin.
    pub fn new(
        api_key:     String,
        source_lang: Option<&str>,
        target_lang: &str,
    ) -> Self {
        let endpoint = if api_key.ends_with(":fx") {
            "https://api-free.deepl.com/v2"
        } else {
            "https://api.deepl.com/v2"
        };
        Self {
            api_key,
            source_lang: source_lang.map(|s| s.to_uppercase()),
            target_lang: target_lang.to_uppercase(),
            model_type:  "latency_optimized".into(),
            endpoint:    endpoint.into(),
            context_sentences: 5,
        }
    }
}

/// Rolling ring of the last N source sentences passed as DeepL `context`.
/// The context window slides: push a new sentence → oldest is evicted.
/// The sentence being translated is NOT included in the context string —
/// it's the `text` param. Context = prior sentences only.
pub struct TranslationContext {
    sentences: std::collections::VecDeque<String>,
    capacity:  usize,
}

impl TranslationContext {
    pub fn new(capacity: usize) -> Self {
        Self {
            sentences: std::collections::VecDeque::with_capacity(capacity + 1),
            capacity,
        }
    }

    /// Return the context string (prior sentences joined with newlines) to
    /// pass to DeepL, then push `sentence` into the ring.
    pub fn push_and_context(&mut self, sentence: &str) -> String {
        let ctx = self.sentences.iter().cloned().collect::<Vec<_>>().join("\n");
        self.sentences.push_back(sentence.to_string());
        if self.sentences.len() > self.capacity {
            self.sentences.pop_front();
        }
        ctx
    }
}

/// Async DeepL client. Reuse across calls — reqwest keeps the connection pool.
pub struct DeepLClient {
    cfg:  DeepLConfig,
    http: reqwest::Client,
}

impl DeepLClient {
    pub fn new(cfg: DeepLConfig) -> Self {
        Self { http: reqwest::Client::new(), cfg }
    }

    /// Translate `text` using `context` (prior sentences; may be empty).
    pub async fn translate(&self, text: &str, context: &str) -> anyhow::Result<String> {
        let url = format!("{}/translate", self.cfg.endpoint);

        let body = TranslateRequest {
            text:        vec![text.to_string()],
            source_lang: self.cfg.source_lang.clone(),
            target_lang: self.cfg.target_lang.clone(),
            model_type:  self.cfg.model_type.clone(),
            context:     if context.is_empty() { None } else { Some(context.to_string()) },
        };

        let t0 = ts();
        log::info!(
            "[{}] DeepL send  text={:?}  ctx={:?}",
            t0, text,
            if context.is_empty() { "(none)" } else { context }
        );
        let sent_at = std::time::Instant::now();

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("DeepL-Auth-Key {}", self.cfg.api_key))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("DeepL HTTP {status}: {err_body}");
        }

        let result: TranslateResponse = resp.json().await?;
        let translated = result.translations.into_iter().next().map(|t| t.text).unwrap_or_default();
        let elapsed_ms = sent_at.elapsed().as_millis();

        log::info!(
            "[{}] DeepL recv +{}ms | {:?} → {:?}",
            t0, elapsed_ms, text, translated
        );

        Ok(translated)
    }
}

fn ts() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let s  = (ms / 1000) % 86400;
    format!("{:02}:{:02}:{:02}.{:03}", s / 3600, (s % 3600) / 60, s % 60, ms % 1000)
}

// --- Wire types ---

#[derive(Debug, Serialize)]
struct TranslateRequest {
    text:        Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_lang: Option<String>,
    target_lang: String,
    model_type:  String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context:     Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranslateResponse {
    translations: Vec<Translation>,
}

#[derive(Debug, Deserialize)]
struct Translation {
    text: String,
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_starts_empty() {
        let mut ctx = TranslationContext::new(4);
        assert_eq!(ctx.push_and_context("hello"), "");
    }

    #[test]
    fn context_accumulates_prior_sentences() {
        let mut ctx = TranslationContext::new(4);
        ctx.push_and_context("sentence one.");
        let c = ctx.push_and_context("sentence two.");
        assert_eq!(c, "sentence one.");
        let c = ctx.push_and_context("sentence three.");
        assert_eq!(c, "sentence one.\nsentence two.");
    }

    #[test]
    fn context_evicts_oldest_when_full() {
        let mut ctx = TranslationContext::new(2);
        ctx.push_and_context("a");
        ctx.push_and_context("b");
        ctx.push_and_context("c");
        let c = ctx.push_and_context("d");
        assert_eq!(c, "b\nc");
    }

    #[test]
    fn free_key_uses_free_endpoint() {
        let cfg = DeepLConfig::new("abc:fx".into(), None, "de");
        assert!(cfg.endpoint.contains("api-free"), "expected free endpoint, got {}", cfg.endpoint);
    }

    #[test]
    fn paid_key_uses_paid_endpoint() {
        let cfg = DeepLConfig::new("abcdef".into(), None, "de");
        assert!(!cfg.endpoint.contains("api-free"), "expected paid endpoint, got {}", cfg.endpoint);
    }

    #[test]
    fn target_lang_uppercased() {
        let cfg = DeepLConfig::new("key".into(), None, "de");
        assert_eq!(cfg.target_lang, "DE");
    }

    #[test]
    fn source_lang_none_by_default() {
        let cfg = DeepLConfig::new("key".into(), None, "de");
        assert!(cfg.source_lang.is_none());
    }

    #[test]
    fn source_lang_explicit_uppercased() {
        let cfg = DeepLConfig::new("key".into(), Some("en"), "de");
        assert_eq!(cfg.source_lang, Some("EN".into()));
    }
}
