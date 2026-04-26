//! UiState — pure data model for the egui layer.
//!
//! No egui imports, no tokio. Receives `SessionEvent`s from the session runner
//! and updates fields that the renderer reads.

use std::time::Instant;

use audio_os::{list_nodes, MediaClass, NodeInfo};
use pipeline::TrackId;

use crate::config::AppConfig;
use crate::session::SessionEvent;

// ── Audio device list ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AudioNode {
    pub id:          u32,
    pub name:        String,
    pub description: String,
    pub class:       MediaClass,
}

impl From<NodeInfo> for AudioNode {
    fn from(n: NodeInfo) -> Self {
        Self { id: n.id, name: n.name, description: n.description, class: n.media_class }
    }
}

// ── Subtitle history ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SubtitleLine {
    #[allow(dead_code)]
    pub track:      TrackId,
    pub source:     String,
    pub translated: String,
    pub ts:         Instant,
}

// ── Session status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Idle,
    Running,
    Stopping,
}

// ── Language list ──────────────────────────────────────────────────────────

pub const SUPPORTED_LANGS: &[(&str, &str)] = &[
    ("EN", "English"),
    ("DE", "German"),
    ("NL", "Dutch"),
    ("IT", "Italian"),
    ("ES", "Spanish"),
    ("FR", "French"),
    ("PL", "Polish"),
    ("PT-PT", "Portuguese"),
    ("RU", "Russian"),
    ("ZH", "Chinese"),
    ("JA", "Japanese"),
    ("KO", "Korean"),
];

// ── Main state struct ──────────────────────────────────────────────────────

pub struct UiState {
    // ── Audio device lists (refreshed on demand) ──────────────────────────
    pub nodes: Vec<AudioNode>,

    // ── User selections ───────────────────────────────────────────────────
    pub selected_mic_idx:    Option<usize>, // index into nodes (Source class)
    pub selected_sink_idx:   Option<usize>, // index into nodes (Sink class) for Track 2
    /// Track 1 (mic → TTS) target language index into SUPPORTED_LANGS.
    pub t1_target_lang_idx:  usize,
    /// Track 2 (incoming audio → subtitles) target language index into SUPPORTED_LANGS.
    pub t2_target_lang_idx:  usize,

    // ── Config fields (editable before session start) ─────────────────────
    pub tts_sink_name:        String,
    pub track2_enabled:       bool,
    pub context_sentences:    usize,

    // Transcript buffer tuning (displayed in "Advanced")
    pub silence_flush_ms:     u32,
    pub min_chars_punct:      usize,
    pub max_chars:            usize,

    // ── API keys ──────────────────────────────────────────────────────────
    pub dg_key:    String,
    pub deepl_key: String,
    pub el_key:    String,
    pub voice_id:  String,

    // ── Overlay ───────────────────────────────────────────────────────────
    /// Number of subtitle lines shown in the overlay window (default 3).
    pub overlay_lines:   usize,

    // ── Runtime ───────────────────────────────────────────────────────────
    pub status:          SessionStatus,
    pub log_path:        Option<std::path::PathBuf>,

    // ── Subtitle history ──────────────────────────────────────────────────
    /// Track 1 outgoing: what the user said and was translated. Shown in main window.
    pub mic_lines:   Vec<SubtitleLine>,
    /// Track 2 incoming: what the other person said. Shown in overlay.
    pub audio_lines: Vec<SubtitleLine>,

    /// Live partial from Track 1 (mic, not yet flushed).
    pub mic_partial:   String,
    /// Live partial from Track 2 (audio, not yet flushed).
    pub audio_partial: String,

    /// Non-fatal error messages (most recent first).
    pub errors: Vec<String>,
}

impl UiState {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let t1_target_lang_idx = SUPPORTED_LANGS
            .iter()
            .position(|(code, _)| *code == cfg.t1_target_lang.as_str())
            .unwrap_or(1); // default to DE
        let t2_target_lang_idx = SUPPORTED_LANGS
            .iter()
            .position(|(code, _)| *code == cfg.t2_target_lang.as_str())
            .unwrap_or(0); // default to EN

        Self {
            nodes: Vec::new(),
            selected_mic_idx:    None,
            selected_sink_idx:   None,
            t1_target_lang_idx,
            t2_target_lang_idx,
            overlay_lines:       3,
            tts_sink_name:       cfg.tts_sink_name.clone().unwrap_or_default(),
            track2_enabled:      cfg.track2_enabled,
            context_sentences:  cfg.context_sentences,
            silence_flush_ms:   cfg.buf.silence_flush.as_millis() as u32,
            min_chars_punct:    cfg.buf.min_chars_for_punct_flush,
            max_chars:          cfg.buf.max_chars_before_flush,
            dg_key:             cfg.dg_api_key.clone(),
            deepl_key:          cfg.deepl_key.clone(),
            el_key:             cfg.el_key.clone(),
            voice_id:           cfg.voice_id.clone(),
            status:             SessionStatus::Idle,
            log_path:           None,
            mic_lines:          Vec::new(),
            audio_lines:        Vec::new(),
            mic_partial:        String::new(),
            audio_partial:      String::new(),
            errors:             Vec::new(),
        }
    }

    /// Populate / refresh the node list from PipeWire.
    pub fn refresh_nodes(&mut self) {
        match list_nodes() {
            Ok(nodes) => {
                self.nodes = nodes.into_iter().map(AudioNode::from).collect();
                // Auto-select first mic if nothing selected yet.
                if self.selected_mic_idx.is_none() {
                    self.selected_mic_idx = self
                        .nodes
                        .iter()
                        .position(|n| matches!(n.class, MediaClass::Source));
                }
                if self.selected_sink_idx.is_none() {
                    self.selected_sink_idx = self
                        .nodes
                        .iter()
                        .position(|n| matches!(n.class, MediaClass::Sink));
                }
            }
            Err(e) => self.push_error(format!("PipeWire node list: {e}")),
        }
    }

    /// Apply an incoming session event.
    pub fn apply_event(&mut self, evt: SessionEvent) {
        match evt {
            SessionEvent::Partial { track, text } => match track {
                TrackId::Outgoing => self.mic_partial = text,
                TrackId::Incoming => self.audio_partial = text,
            },

            SessionEvent::Flushed { track, source: _ } => {
                match track {
                    TrackId::Outgoing => self.mic_partial.clear(),
                    TrackId::Incoming => self.audio_partial.clear(),
                }
                // Do not push a placeholder — only Translated events add subtitle lines.
            }

            SessionEvent::Translated { track, source, translated } => {
                // Update or append the subtitle line for this source.
                let lines = match track {
                    TrackId::Outgoing => &mut self.mic_lines,
                    TrackId::Incoming => &mut self.audio_lines,
                };
                // Replace the last line that has the same source text.
                if let Some(existing) = lines.iter_mut().rev().find(|l| l.source == source) {
                    existing.translated = translated;
                    existing.ts = Instant::now();
                } else {
                    lines.push(SubtitleLine {
                        track,
                        source,
                        translated,
                        ts: Instant::now(),
                    });
                }
                self.trim_lines();
            }

            SessionEvent::Error { track: _, message } => {
                self.push_error(message);
            }

            SessionEvent::Ended { .. } => {
                if self.status == SessionStatus::Stopping || self.status == SessionStatus::Running {
                    self.status = SessionStatus::Idle;
                }
                self.mic_partial.clear();
                self.audio_partial.clear();
            }
        }
    }

    /// Track 1 target language code (mic → TTS), e.g. `"DE"`.
    pub fn t1_target_lang(&self) -> &str {
        SUPPORTED_LANGS
            .get(self.t1_target_lang_idx)
            .map(|(code, _)| *code)
            .unwrap_or("DE")
    }

    /// Track 2 target language code (incoming audio → subtitles), e.g. `"EN"`.
    pub fn t2_target_lang(&self) -> &str {
        SUPPORTED_LANGS
            .get(self.t2_target_lang_idx)
            .map(|(code, _)| *code)
            .unwrap_or("EN")
    }

    /// Selected mic node id.
    pub fn mic_node_id(&self) -> Option<u32> {
        self.selected_mic_idx
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.id)
    }

    /// Selected sink node id.
    pub fn sink_node_id(&self) -> Option<u32> {
        self.selected_sink_idx
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.id)
    }

    // ── Private helpers ────────────────────────────────────────────────────

    fn trim_lines(&mut self) {
        const MAX: usize = 100;
        if self.mic_lines.len() > MAX {
            self.mic_lines.drain(0..self.mic_lines.len() - MAX);
        }
        if self.audio_lines.len() > MAX {
            self.audio_lines.drain(0..self.audio_lines.len() - MAX);
        }
    }

    fn push_error(&mut self, msg: String) {
        log::error!("{msg}");
        self.errors.insert(0, msg);
        if self.errors.len() > 10 {
            self.errors.truncate(10);
        }
    }
}
