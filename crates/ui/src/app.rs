//! TranslatorApp — eframe App implementation.
//!
//! Owns the UiState and a running SessionHandle (when active).
//! Drains SessionEvents each frame, updates state, triggers repaints.
//!
//! Two viewports:
//!   - Central panel: control window (mic picker, languages, keys, status).
//!   - Immediate child viewport: subtitle overlay (borderless, always-on-top).

use std::sync::Arc;
use std::time::Duration;

use egui::{
    Color32, Context, FontId, RichText, ScrollArea, Vec2,
    ViewportBuilder, ViewportId,
};
use pipeline::TranscriptBufferConfig;

use crate::config::AppConfig;
use crate::session::{start_session, SessionHandle};
use crate::state::{SessionStatus, SubtitleLine, UiState, SUPPORTED_LANGS};

// ── App ────────────────────────────────────────────────────────────────────

pub struct TranslatorApp {
    cfg:     AppConfig,
    state:   UiState,
    session: Option<SessionHandle>,
    rt:      Arc<tokio::runtime::Runtime>,
    overlay_id: ViewportId,
}

impl TranslatorApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: AppConfig,
        rt:  Arc<tokio::runtime::Runtime>,
    ) -> Self {
        let mut state = UiState::from_config(&cfg);
        state.refresh_nodes();
        cc.egui_ctx.set_pixels_per_point(1.2);

        Self {
            cfg,
            state,
            session: None,
            rt,
            overlay_id: ViewportId::from_hash_of("subtitle-overlay"),
        }
    }

    // ── Session lifecycle ──────────────────────────────────────────────────

    fn start(&mut self) {
        if self.state.dg_key.is_empty() {
            self.state.errors.insert(0, "DEEPGRAM_API_KEY is required.".into());
            return;
        }

        // Build a live AppConfig from what the user typed in the UI.
        let mut cfg = self.cfg.clone();
        cfg.dg_api_key      = self.state.dg_key.clone();
        cfg.deepl_key       = self.state.deepl_key.clone();
        cfg.el_key          = self.state.el_key.clone();
        cfg.voice_id        = self.state.voice_id.clone();
        cfg.t1_target_lang  = self.state.t1_target_lang().to_owned();
        cfg.t2_target_lang  = self.state.t2_target_lang().to_owned();
        cfg.track2_enabled  = self.state.track2_enabled;
        cfg.context_sentences = self.state.context_sentences;
        cfg.tts_sink_name = if self.state.tts_sink_name.trim().is_empty() {
            None
        } else {
            Some(self.state.tts_sink_name.trim().to_owned())
        };
        cfg.buf = TranscriptBufferConfig {
            silence_flush: Duration::from_millis(self.state.silence_flush_ms as u64),
            min_chars_for_punct_flush: self.state.min_chars_punct,
            max_chars_before_flush:    self.state.max_chars,
            flush_on_utterance_end:    true,
        };

        let t1_lang = self.state.t1_target_lang().to_owned();
        let t2_lang = self.state.t2_target_lang().to_owned();
        match start_session(
            &cfg,
            self.state.mic_node_id(),
            self.state.sink_node_id(),
            &t1_lang,
            &t2_lang,
            self.rt.clone(),
        ) {
            Ok(handle) => {
                self.state.log_path = Some(handle.log_path.clone());
                self.state.status   = SessionStatus::Running;
                self.state.errors.clear();
                self.state.mic_lines.clear();
                self.state.audio_lines.clear();
                self.state.mic_partial.clear();
                self.state.audio_partial.clear();
                self.session = Some(handle);
            }
            Err(e) => {
                self.state.errors.insert(0, format!("Start failed: {e}"));
            }
        }
    }

    fn stop(&mut self) {
        if let Some(ref h) = self.session {
            h.stop();
        }
        self.state.status = SessionStatus::Stopping;
    }

    // ── Event drain ────────────────────────────────────────────────────────

    fn drain_events(&mut self) {
        let Some(ref mut handle) = self.session else { return };
        for _ in 0..64 {
            match handle.event_rx.try_recv() {
                Ok(evt) => self.state.apply_event(evt),
                Err(_)  => break,
            }
        }
        // Clean up once both tracks have ended.
        if self.state.status == SessionStatus::Idle {
            self.session = None;
        }
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────

impl eframe::App for TranslatorApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        if self.state.status == SessionStatus::Running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // ── Control window ─────────────────────────────────────────────────
        let action = egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Realtime Translator");
            ui.separator();
            draw_audio_config(ui, &mut self.state);
            ui.separator();
            draw_api_keys(ui, &mut self.state);
            ui.separator();
            draw_advanced(ui, &mut self.state);
            ui.separator();
            let action = draw_session_controls(ui, &self.state);
            ui.separator();
            draw_status(ui, &self.state);
            ui.separator();
            draw_mic_history(ui, &self.state);
            action
        }).inner;

        match action {
            SessionAction::Start => self.start(),
            SessionAction::Stop  => self.stop(),
            SessionAction::None  => {}
        }

        // ── Subtitle overlay (Track 2 incoming) ───────────────────────────
        if self.state.track2_enabled
            && (self.state.status == SessionStatus::Running
                || !self.state.audio_lines.is_empty())
        {
            let audio_lines   = self.state.audio_lines.clone();
            let audio_partial = self.state.audio_partial.clone();
            let overlay_lines = self.state.overlay_lines;

            ctx.show_viewport_immediate(
                self.overlay_id,
                ViewportBuilder::default()
                    .with_title("Translator Subtitles")
                    .with_inner_size(Vec2::new(860.0, 160.0))
                    .with_always_on_top()
                    .with_decorations(true),
                move |ctx, _| {
                    draw_subtitle_overlay(ctx, &audio_lines, &audio_partial, overlay_lines);
                },
            );
        }
    }
}

// ── Control window sections ────────────────────────────────────────────────

fn draw_audio_config(ui: &mut egui::Ui, state: &mut UiState) {
    let running = state.status != SessionStatus::Idle;

    ui.collapsing("Audio Sources", |ui| {
        egui::Grid::new("audio_grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                // Mic picker
                ui.label("Microphone:");
                egui::ComboBox::from_id_salt("mic_pick")
                    .selected_text(node_label(state, state.selected_mic_idx))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.selected_mic_idx, None, "Default");
                        let source_idxs: Vec<usize> = state.nodes.iter().enumerate()
                            .filter(|(_, n)| matches!(n.class, audio_os::MediaClass::Source))
                            .map(|(i, _)| i)
                            .collect();
                        for i in source_idxs {
                            let label = format!("{} ({})", state.nodes[i].description, state.nodes[i].name);
                            ui.selectable_value(&mut state.selected_mic_idx, Some(i), label);
                        }
                    });
                ui.end_row();

                // Track 1 target language (mic → TTS)
                ui.label("Mic translates to:");
                ui.add_enabled_ui(!running, |ui| {
                    egui::ComboBox::from_id_salt("t1_target_lang")
                        .selected_text(SUPPORTED_LANGS[state.t1_target_lang_idx].1)
                        .show_ui(ui, |ui| {
                            for (i, (_, name)) in SUPPORTED_LANGS.iter().enumerate() {
                                ui.selectable_value(&mut state.t1_target_lang_idx, i, *name);
                            }
                        });
                });
                ui.end_row();

                // Track 2 toggle
                ui.label("Incoming subtitles (Track 2):");
                ui.add_enabled(!running, egui::Checkbox::new(&mut state.track2_enabled, ""));
                ui.end_row();

                if state.track2_enabled {
                    ui.label("Audio source:");
                    egui::ComboBox::from_id_salt("sink_pick")
                        .selected_text(node_label(state, state.selected_sink_idx))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut state.selected_sink_idx, None, "Default sink monitor");
                            let sink_idxs: Vec<usize> = state.nodes.iter().enumerate()
                                .filter(|(_, n)| matches!(n.class, audio_os::MediaClass::Sink))
                                .map(|(i, _)| i)
                                .collect();
                            for i in sink_idxs {
                                let label = format!("{} ({})", state.nodes[i].description, state.nodes[i].name);
                                ui.selectable_value(&mut state.selected_sink_idx, Some(i), label);
                            }
                        });
                    ui.end_row();

                    // Track 2 target language (incoming audio → subtitles)
                    ui.label("Incoming translates to:");
                    ui.add_enabled_ui(!running, |ui| {
                        egui::ComboBox::from_id_salt("t2_target_lang")
                            .selected_text(SUPPORTED_LANGS[state.t2_target_lang_idx].1)
                            .show_ui(ui, |ui| {
                                for (i, (_, name)) in SUPPORTED_LANGS.iter().enumerate() {
                                    ui.selectable_value(&mut state.t2_target_lang_idx, i, *name);
                                }
                            });
                    });
                    ui.end_row();
                }

                // TTS output sink
                ui.label("TTS output sink:");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut state.tts_sink_name)
                        .hint_text("translator_virtmic_sink  (blank = default)"),
                );
                ui.end_row();
            });

        if ui.button("Refresh devices").clicked() {
            state.refresh_nodes();
        }
    });
}

fn draw_api_keys(ui: &mut egui::Ui, state: &mut UiState) {
    let running = state.status != SessionStatus::Idle;

    ui.collapsing("API Keys", |ui| {
        egui::Grid::new("keys_grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Deepgram (required):");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut state.dg_key).password(true),
                );
                ui.end_row();

                ui.label("DeepL (translation):");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut state.deepl_key).password(true),
                );
                ui.end_row();

                ui.label("ElevenLabs (TTS):");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut state.el_key).password(true),
                );
                ui.end_row();

                ui.label("Voice ID:");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut state.voice_id),
                );
                ui.end_row();
            });
    });
}

fn draw_advanced(ui: &mut egui::Ui, state: &mut UiState) {
    let running = state.status != SessionStatus::Idle;

    ui.collapsing("Advanced", |ui| {
        egui::Grid::new("adv_grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Silence flush (ms):");
                let mut s = state.silence_flush_ms.to_string();
                if ui.add_enabled(!running, egui::TextEdit::singleline(&mut s).desired_width(60.0)).changed() {
                    if let Ok(v) = s.parse::<u32>() { state.silence_flush_ms = v.max(100); }
                }
                ui.end_row();

                ui.label("Min chars (punct):");
                let mut s = state.min_chars_punct.to_string();
                if ui.add_enabled(!running, egui::TextEdit::singleline(&mut s).desired_width(60.0)).changed() {
                    if let Ok(v) = s.parse::<usize>() { state.min_chars_punct = v; }
                }
                ui.end_row();

                ui.label("Max chars:");
                let mut s = state.max_chars.to_string();
                if ui.add_enabled(!running, egui::TextEdit::singleline(&mut s).desired_width(60.0)).changed() {
                    if let Ok(v) = s.parse::<usize>() { state.max_chars = v.max(50); }
                }
                ui.end_row();

                ui.label("Context sentences:");
                let mut s = state.context_sentences.to_string();
                if ui.add_enabled(!running, egui::TextEdit::singleline(&mut s).desired_width(60.0)).changed() {
                    if let Ok(v) = s.parse::<usize>() { state.context_sentences = v.max(1).min(20); }
                }
                ui.end_row();

                ui.label("Overlay lines shown:");
                let mut s = state.overlay_lines.to_string();
                if ui.add(egui::TextEdit::singleline(&mut s).desired_width(60.0)).changed() {
                    if let Ok(v) = s.parse::<usize>() { state.overlay_lines = v.max(1).min(10); }
                }
                ui.end_row();
            });
    });
}

enum SessionAction { Start, Stop, None }

fn draw_session_controls(ui: &mut egui::Ui, state: &UiState) -> SessionAction {
    let mut action = SessionAction::None;

    ui.horizontal(|ui| {
        match state.status {
            SessionStatus::Idle => {
                if ui.button(
                    RichText::new("▶  Start").color(Color32::from_rgb(80, 200, 80))
                ).clicked() {
                    action = SessionAction::Start;
                }
            }
            SessionStatus::Running => {
                if ui.button(
                    RichText::new("■  Stop").color(Color32::from_rgb(220, 80, 80))
                ).clicked() {
                    action = SessionAction::Stop;
                }
                ui.spinner();
                ui.label(RichText::new("Listening…").color(Color32::from_rgb(80, 200, 80)));
            }
            SessionStatus::Stopping => {
                ui.spinner();
                ui.label(RichText::new("Stopping…").color(Color32::GRAY));
            }
        }
    });

    if let Some(p) = &state.log_path {
        ui.label(
            RichText::new(format!("Log: {}", p.display()))
                .small()
                .color(Color32::GRAY),
        );
    }

    action
}

fn draw_status(ui: &mut egui::Ui, state: &UiState) {
    for e in state.errors.iter().take(3) {
        ui.colored_label(Color32::from_rgb(220, 80, 80), format!("⚠ {e}"));
    }

    if !state.mic_partial.is_empty() {
        ui.label(
            RichText::new(format!("🎤 …{}", state.mic_partial))
                .color(Color32::GRAY)
                .italics()
                .small(),
        );
    }
}

fn draw_mic_history(ui: &mut egui::Ui, state: &UiState) {
    ui.label(RichText::new("Your speech (Track 1)").small().color(Color32::GRAY));

    ScrollArea::vertical()
        .max_height(180.0)
        .id_salt("mic_scroll")
        .show(ui, |ui| {
            let n = state.mic_lines.len();
            let start = n.saturating_sub(8);
            for line in &state.mic_lines[start..] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(&line.translated)
                            .font(FontId::proportional(14.0)),
                    );
                });
                if !line.source.is_empty() && line.source != line.translated {
                    ui.label(
                        RichText::new(format!("  ↑ {}", line.source))
                            .small()
                            .color(Color32::GRAY),
                    );
                }
            }
        });
}

// ── Subtitle overlay ───────────────────────────────────────────────────────

fn draw_subtitle_overlay(
    ctx:          &Context,
    lines:        &[SubtitleLine],
    partial:      &str,
    overlay_lines: usize,
) {
    let bg = Color32::from_rgba_premultiplied(0, 0, 0, 210);

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(bg).inner_margin(egui::Margin::same(10)))
        .show(ctx, |ui| {
            ui.set_min_size(ui.available_size());

            let n = lines.len();
            let start = n.saturating_sub(overlay_lines);
            for line in &lines[start..] {
                let age   = line.ts.elapsed().as_secs_f32();
                let alpha = ((1.0 - age / 10.0).max(0.0) * 255.0) as u8;
                ui.label(
                    RichText::new(&line.translated)
                        .color(Color32::from_rgba_premultiplied(255, 255, 255, alpha))
                        .font(FontId::proportional(22.0))
                        .strong(),
                );
            }

            if !partial.is_empty() {
                ui.label(
                    RichText::new(format!("…{partial}"))
                        .color(Color32::from_rgba_premultiplied(200, 200, 200, 160))
                        .font(FontId::proportional(17.0))
                        .italics(),
                );
            }
        });
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn node_label(state: &UiState, idx: Option<usize>) -> String {
    match idx {
        None => "Default".to_owned(),
        Some(i) => state.nodes.get(i)
            .map(|n| format!("{} ({})", n.description, n.name))
            .unwrap_or_else(|| "?".to_owned()),
    }
}
