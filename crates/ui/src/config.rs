//! AppConfig — all user-configurable settings.
//!
//! Sources (applied in order, later overrides earlier):
//!   1. Built-in defaults (in `Default`)
//!   2. `~/.config/realtime-translation/config.toml` (if present)
//!   3. Environment variables / `.env` file
//!
//! The `config.toml` format mirrors this struct with optional fields.
//! Missing fields keep the default. Unknown fields are silently ignored.

use std::path::PathBuf;
use std::time::Duration;

use pipeline::TranscriptBufferConfig;
use serde::Deserialize;

// ── Top-level config ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AppConfig {
    // ── API keys (secrets — from env only, never stored in config.toml) ──
    pub dg_api_key:   String,
    pub deepl_key:    String,
    pub el_key:       String,
    pub voice_id:     String,

    // ── Language ──────────────────────────────────────────────────────────
    /// Deepgram STT source language. `None` = auto-detect (`language=multi`).
    pub source_lang:     Option<String>,
    /// DeepL target language for Track 1 (mic → TTS). e.g. `"DE"`.
    pub t1_target_lang:  String,
    /// DeepL target language for Track 2 (incoming audio → subtitles). e.g. `"EN"`.
    pub t2_target_lang:  String,

    // ── Track 1 (outgoing mic) ────────────────────────────────────────────
    /// `node.name` of the PipeWire sink to route TTS output into.
    /// `None` = system default sink (headphones).
    pub tts_sink_name: Option<String>,

    // ── Track 2 (incoming audio → subtitles) ─────────────────────────────
    pub track2_enabled: bool,

    // ── Transcript buffer ─────────────────────────────────────────────────
    pub buf: TranscriptBufferConfig,

    /// Number of prior source sentences sent to DeepL as context.
    pub context_sentences: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            dg_api_key:        String::new(),
            deepl_key:         String::new(),
            el_key:            String::new(),
            voice_id:          String::new(),
            source_lang:       None,
            t1_target_lang:    "DE".to_owned(),
            t2_target_lang:    "EN".to_owned(),
            tts_sink_name:     Some("translator_virtmic_sink".to_owned()),
            track2_enabled:    true,
            buf:               TranscriptBufferConfig::default(),
            context_sentences: 5,
        }
    }
}

impl AppConfig {
    /// Load config: defaults → config.toml → env vars.
    pub fn load() -> Self {
        let _ = dotenvy::dotenv(); // load .env if present; ignore errors

        let mut cfg = Self::default();

        // Layer 2: config.toml
        if let Some(path) = config_toml_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => match toml::from_str::<TomlFile>(&text) {
                        Ok(t) => cfg.apply_toml(t),
                        Err(e) => log::warn!("config.toml parse error (ignored): {e}"),
                    },
                    Err(e) => log::warn!("could not read config.toml (ignored): {e}"),
                }
            }
        }

        // Layer 3: env vars (overrides toml)
        if let Ok(v) = std::env::var("DEEPGRAM_API_KEY") { cfg.dg_api_key = v; }
        if let Ok(v) = std::env::var("DEEPL_API_KEY")    { cfg.deepl_key  = v; }
        if let Ok(v) = std::env::var("ELEVENLABS_API_KEY") { cfg.el_key   = v; }
        if let Ok(v) = std::env::var("VOICE_ID")          { cfg.voice_id  = v; }

        cfg
    }

    fn apply_toml(&mut self, t: TomlFile) {
        if let Some(lang) = t.t1_target_lang { self.t1_target_lang = lang; }
        if let Some(lang) = t.t2_target_lang { self.t2_target_lang = lang; }
        if let Some(lang) = t.source_lang    { self.source_lang = Some(lang); }
        if let Some(sink) = t.tts_sink_name { self.tts_sink_name = Some(sink); }
        if let Some(v) = t.track2_enabled   { self.track2_enabled = v; }
        if let Some(v) = t.context_sentences { self.context_sentences = v; }

        if let Some(b) = t.buffer {
            if let Some(v) = b.min_chars_for_punct_flush {
                self.buf.min_chars_for_punct_flush = v;
            }
            if let Some(v) = b.max_chars_before_flush {
                self.buf.max_chars_before_flush = v;
            }
            if let Some(v) = b.silence_flush_ms {
                self.buf.silence_flush = Duration::from_millis(v as u64);
            }
            if let Some(v) = b.flush_on_utterance_end {
                self.buf.flush_on_utterance_end = v;
            }
        }
    }

    pub fn has_deepl(&self) -> bool  { !self.deepl_key.is_empty() }
    pub fn has_tts(&self)   -> bool  { !self.el_key.is_empty() && !self.voice_id.is_empty() }
}

// ── TOML deserialization schema ────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct TomlFile {
    t1_target_lang:   Option<String>,
    t2_target_lang:   Option<String>,
    source_lang:      Option<String>,
    tts_sink_name:    Option<String>,
    track2_enabled:   Option<bool>,
    context_sentences: Option<usize>,
    buffer:           Option<TomlBuffer>,
}

#[derive(Debug, Deserialize)]
struct TomlBuffer {
    min_chars_for_punct_flush: Option<usize>,
    max_chars_before_flush:    Option<usize>,
    silence_flush_ms:          Option<u32>,
    flush_on_utterance_end:    Option<bool>,
}

// ── Paths ──────────────────────────────────────────────────────────────────

pub fn config_toml_path() -> Option<PathBuf> {
    dirs_path().map(|d| d.join("config.toml"))
}

pub fn sessions_dir() -> Option<PathBuf> {
    data_dir_path().map(|d| d.join("sessions"))
}

fn dirs_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("realtime-translation")
    })
}

fn data_dir_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("share")
            .join("realtime-translation")
    })
}
