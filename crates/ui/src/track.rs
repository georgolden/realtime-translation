//! Single-track pipeline runner.
//!
//! One track = one Deepgram WS + one TranscriptBuffer + optional DeepL +
//! optional ElevenLabs → PipeWire sink.
//!
//! Outgoing (Track 1):  mic capture → STT → translate → TTS → virtmic
//! Incoming (Track 2):  sink-monitor capture → STT → translate → subtitles only
//!
//! Both tracks emit the same `TrackEvent` type. The session layer owns two
//! tracks and merges their events.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use audio_os::{capture_indefinite, CaptureTarget, PlaybackFormat, PlaybackTarget};
use pipeline::{
    DeepgramClient, DeepgramConfig, DeepLClient, DeepLConfig, ElevenLabsConfig,
    PipelineEvent, ResampleState, TrackId, TranslationContext,
};
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::transcript::{LogTrack, TranscriptLog};

// ── Public types ───────────────────────────────────────────────────────────

/// Events emitted by a single track, consumed by UiState.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TrackEvent {
    /// Live in-flight transcript (not committed yet).
    Partial { track: TrackId, text: String },
    /// Live in-flight transcript translated (throttled, may be stale).
    PartialTranslated { track: TrackId, source: String, translated: String, seq: u64 },
    /// Flushed source text, translation pending.
    Flushed { track: TrackId, source: String },
    /// Translation arrived for a flushed chunk.
    Translated { track: TrackId, source: String, translated: String },
    /// Non-fatal error; track keeps running.
    Error { track: TrackId, message: String },
    /// Track has ended (capture stopped, or fatal WS error after retries).
    Ended { track: TrackId },
}

/// Identifies how the track captures audio.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TrackSource {
    /// Microphone (Track 1). `None` = PipeWire default source.
    Mic(Option<u32>),
    /// Sink monitor — what a sink is playing (Track 2). `None` = default sink.
    SinkMonitor(Option<u32>),
}

impl TrackSource {
    #[allow(dead_code)]
    pub fn to_capture_target(&self) -> CaptureTarget {
        resolve_capture_target(self)
    }

    #[allow(dead_code)]
    pub fn is_sink_monitor(&self) -> bool {
        matches!(self, TrackSource::SinkMonitor(_))
    }

    pub fn log_track(&self) -> LogTrack {
        match self {
            TrackSource::Mic(_) => LogTrack::Mic,
            TrackSource::SinkMonitor(_) => LogTrack::Audio,
        }
    }
}

/// Configuration for one track.
#[derive(Debug, Clone)]
pub struct TrackConfig {
    pub track_id:  TrackId,
    pub source:    TrackSource,

    // STT
    pub dg_api_key:  String,
    pub source_lang: Option<String>, // None = Deepgram auto-detect
    // Translation (optional)
    pub deepl:       Option<DeeplTrackConfig>,

    // TTS + playback (outgoing only; None on incoming)
    pub tts:         Option<TtsConfig>,
}

#[derive(Debug, Clone)]
pub struct DeeplTrackConfig {
    pub api_key:           String,
    pub source_lang:       Option<String>,
    pub target_lang:       String,
    pub context_sentences: usize,
}

#[derive(Debug, Clone)]
pub struct TtsConfig {
    pub el_api_key:    String,
    pub voice_id:      String,
    pub sink_name:     Option<String>, // None = default sink
}

/// Build `TrackConfig` values from AppConfig for both tracks.
///
/// `t1_target_lang` feeds Track 1 (mic → TTS); `t2_target_lang` feeds Track 2
/// (incoming audio → subtitles). Each is an independent DeepL target.
pub fn track_configs_from_app(
    cfg:             &AppConfig,
    mic_node:        Option<u32>,
    sink_node:       Option<u32>,
    t1_target_lang:  &str,
    t2_target_lang:  &str,
) -> (TrackConfig, Option<TrackConfig>) {
    let make_deepl = |target_lang: &str| -> Option<DeeplTrackConfig> {
        if cfg.has_deepl() {
            Some(DeeplTrackConfig {
                api_key:           cfg.deepl_key.clone(),
                source_lang:       cfg.source_lang.clone(),
                target_lang:       target_lang.to_owned(),
                context_sentences: cfg.context_sentences,
            })
        } else {
            None
        }
    };

    let tts_cfg = if cfg.has_tts() {
        Some(TtsConfig {
            el_api_key: cfg.el_key.clone(),
            voice_id:   cfg.voice_id.clone(),
            sink_name:  cfg.tts_sink_name.clone(),
        })
    } else {
        None
    };

    let t1 = TrackConfig {
        track_id:    TrackId::Outgoing,
        source:      TrackSource::Mic(mic_node),
        dg_api_key:  cfg.dg_api_key.clone(),
        source_lang: cfg.source_lang.clone(),
        deepl:       make_deepl(t1_target_lang),
        tts:         tts_cfg,
    };

    let t2 = if cfg.track2_enabled && cfg.has_deepl() {
        Some(TrackConfig {
            track_id:    TrackId::Incoming,
            source:      TrackSource::SinkMonitor(sink_node),
            dg_api_key:  cfg.dg_api_key.clone(),
            source_lang: cfg.source_lang.clone(),
            deepl:       make_deepl(t2_target_lang),
            tts:         None,
        })
    } else {
        None
    };

    (t1, t2)
}

// ── Track spawn ────────────────────────────────────────────────────────────

/// Spawn a track. Returns a receiver for events and a stop handle.
/// The track runs until `stop` is set or a fatal error occurs.
pub fn spawn_track(
    cfg:    TrackConfig,
    stop:   Arc<AtomicBool>,
    log:    TranscriptLog,
    rt:     Arc<tokio::runtime::Runtime>,
) -> mpsc::Receiver<TrackEvent> {
    let (event_tx, event_rx) = mpsc::channel::<TrackEvent>(256);

    rt.spawn(async move {
        if let Err(e) = run_track(cfg, stop, log, event_tx.clone()).await {
            let _ = event_tx.send(TrackEvent::Error {
                track:   TrackId::Outgoing, // overridden inside run_track on error
                message: format!("{e}"),
            }).await;
        }
    });

    event_rx
}

async fn run_track(
    cfg:      TrackConfig,
    stop:     Arc<AtomicBool>,
    log:      TranscriptLog,
    event_tx: mpsc::Sender<TrackEvent>,
) -> anyhow::Result<()> {
    let track_id = cfg.track_id;
    let log_track = cfg.source.log_track();

    // ── Deepgram ───────────────────────────────────────────────────────────
    let dg_cfg = match cfg.source_lang.as_deref() {
        Some(lang) => DeepgramConfig::with_language(cfg.dg_api_key.clone(), lang),
        None       => DeepgramConfig::with_detect_language(cfg.dg_api_key.clone()),
    };
    let (dg_handle, mut dg_events) = DeepgramClient::spawn(dg_cfg, track_id);
    let dg_handle = Arc::new(dg_handle);

    // ── DeepL ──────────────────────────────────────────────────────────────
    let deepl_state: Option<(DeepLClient, String, TranslationContext)> =
        cfg.deepl.as_ref().map(|dc| {
            let mut dl_cfg = DeepLConfig::new(
                dc.api_key.clone(),
                dc.source_lang.as_deref(),
                &dc.target_lang,
            );
            dl_cfg.context_sentences = dc.context_sentences;
            let target_lang = dc.target_lang.clone();
            let ctx = TranslationContext::new(dc.context_sentences);
            (DeepLClient::new(dl_cfg), target_lang, ctx)
        });

    // ── ElevenLabs + PipeWire playback (outgoing only) ────────────────────
    let el_tx: Option<mpsc::Sender<String>> = if let Some(ref tts) = cfg.tts {
        let el_cfg = ElevenLabsConfig::new(tts.el_api_key.clone(), tts.voice_id.clone());
        let sample_rate = el_cfg.sample_rate();
        let (text_tx, mut pcm_rx) = pipeline::elevenlabs_spawn(el_cfg);

        let pb_target = match tts.sink_name.as_deref() {
            Some(name) => PlaybackTarget::NodeName(name.to_owned()),
            None       => PlaybackTarget::Default,
        };
        let pb_format = PlaybackFormat { sample_rate, channels: 1 };
        const RING_CAP: usize = 24_000 * 30;

        let (mut pw_handle, pw_join) =
            audio_os::spawn_streaming_player(pb_target, pb_format, RING_CAP);

        tokio::spawn(async move {
            while let Some(msg) = pcm_rx.recv().await {
                if let Some(samples) = msg {
                    pw_handle.push_pcm(&samples);
                }
            }
            pw_handle.finish();
            let _ = tokio::task::spawn_blocking(move || pw_join.join()).await;
        });

        Some(text_tx)
    } else {
        None
    };

    // ── Capture thread ─────────────────────────────────────────────────────
    // Runs on a blocking OS thread; pushes resampled i16 to Deepgram.
    let capture_target = resolve_capture_target(&cfg.source);
    let dg_h = dg_handle.clone();
    let stop_cap = stop.clone();

    tokio::task::spawn_blocking(move || {
        let mut state: Option<(ResampleState, u32, u16)> = None; // (resampler, rate, channels)
        if let Err(e) = capture_indefinite(
            capture_target,
            stop_cap,
            move |samples, fmt| {
                // Re-initialize resampler if the format changed (e.g. after a
                // default-device switch in pavucontrol renegotiates the stream).
                let needs_reinit = state.as_ref()
                    .map(|(_, r, c)| *r != fmt.sample_rate || *c != fmt.channels)
                    .unwrap_or(true);
                if needs_reinit {
                    match ResampleState::new(fmt.sample_rate, fmt.channels) {
                        Ok(rs) => {
                            log::info!("capture: resampler (re)init rate={} ch={}", fmt.sample_rate, fmt.channels);
                            state = Some((rs, fmt.sample_rate, fmt.channels));
                        }
                        Err(e) => {
                            log::error!("capture: resampler init failed: {e}");
                            return;
                        }
                    }
                }
                let (rs, _, _) = state.as_mut().unwrap();
                if let Ok(pcm16) = rs.push(samples) {
                    if !pcm16.is_empty() {
                        dg_h.push_pcm(pcm16);
                    }
                }
            },
        ) {
            log::error!("capture_indefinite: {e}");
        }
        log::info!("capture thread for track {:?} ended", track_id);
    });

    // ── Partial translation task (completely isolated from is_final pipeline) ─
    let (partial_tx, mut partial_rx) = mpsc::channel::<String>(16);
    if let Some((dl, _, _)) = deepl_state.as_ref() {
        let dl = dl.clone();
        let tx = event_tx.clone();
        let tid = track_id;
        tokio::spawn(async move {
            let mut last_tx = Instant::now() - Duration::from_secs(10); // allow immediate first
            let mut seq = 0u64;
            let mut current: Option<String> = None;
            loop {
                // Wait for new partial or a periodic 300 ms tick.
                let recv = tokio::time::timeout(Duration::from_millis(300), partial_rx.recv()).await;
                match recv {
                    Ok(Some(text)) => current = Some(text),
                    Ok(None) => break, // channel closed
                    Err(_) => {}      // timer fired — attempt translation if we have text
                }

                let now = Instant::now();
                if now.duration_since(last_tx) < Duration::from_millis(300) {
                    continue;
                }

                if let Some(text) = current.take() {
                    if text.trim().len() < 5 {
                        continue;
                    }
                    last_tx = now;
                    seq += 1;
                    let dl = dl.clone();
                    let tx = tx.clone();
                    let t = text;
                    let s = seq;
                    // Fire the DeepL call in its own sub-task so the loop stays
                    // responsive and can absorb newer partials while translating.
                    tokio::spawn(async move {
                        match dl.translate(&t, "").await {
                            Ok(translated) => {
                                let _ = tx.send(TrackEvent::PartialTranslated {
                                    track: tid,
                                    source: t,
                                    translated,
                                    seq: s,
                                }).await;
                            }
                            Err(e) => {
                                log::debug!("Partial translation failed (ignored): {e}");
                            }
                        }
                    });
                }
            }
        });
    }

    // ── Event + translation loop ───────────────────────────────────────────
    let mut deepl_state = deepl_state;

    loop {
        // Check stop flag — if set, drop dg_handle to close the WS.
        if stop.load(Ordering::Acquire) {
            log::info!("Track {:?}: stop flag set, closing Deepgram WS", track_id);
            break;
        }

        match tokio::time::timeout(Duration::from_millis(100), dg_events.recv()).await {
            Err(_timeout) => {
                // No event in 100ms — re-check stop flag on next iteration.
                continue;
            }
            Ok(None) => {
                // Channel closed (Deepgram WS ended).
                break;
            }
            Ok(Some(evt)) => {
                handle_pipeline_event(
                    evt,
                    track_id,
                    log_track,
                    &log,
                    &event_tx,
                    &mut deepl_state,
                    &el_tx,
                    &partial_tx,
                ).await;
            }
        }
    }

    drop(partial_tx); // signal partial task to shut down
    let _ = event_tx.send(TrackEvent::Ended { track: track_id }).await;
    drop(el_tx);
    log::info!("Track {:?}: runner exited", track_id);
    Ok(())
}

async fn handle_pipeline_event(
    evt:          PipelineEvent,
    track_id:     TrackId,
    log_track:    LogTrack,
    log:          &TranscriptLog,
    event_tx:     &mpsc::Sender<TrackEvent>,
    deepl_state:  &mut Option<(DeepLClient, String, TranslationContext)>,
    el_tx:        &Option<mpsc::Sender<String>>,
    partial_tx:   &mpsc::Sender<String>,
) {
    match evt {
        PipelineEvent::Partial { text, .. } => {
            log.log_partial(log_track, &text);
            let _ = event_tx.send(TrackEvent::Partial { track: track_id, text: text.clone() }).await;
            // Forward to the isolated partial-translation task.  It decides
            // when to actually call DeepL; we never block here.
            let _ = partial_tx.try_send(text);
        }

        PipelineEvent::Finalised { .. } => {
            // Not surfaced to UI — Flushed is the useful event.
        }

        PipelineEvent::Flushed { text, .. } => {
            log.log_source(log_track, &text);
            let _ = event_tx.send(TrackEvent::Flushed {
                track: track_id,
                source: text.clone(),
            }).await;

            // Translation
            if let Some((dl, target_lang, ctx)) = deepl_state.as_mut().map(|(a, b, c)| (&*a, b.as_str(), c)) {
                let context = ctx.push_and_context(&text);
                match dl.translate(&text, &context).await {
                    Ok(translated) => {
                        log.log_translated(log_track, target_lang, &translated);
                        let _ = event_tx.send(TrackEvent::Translated {
                            track: track_id,
                            source: text.clone(),
                            translated: translated.clone(),
                        }).await;
                        if let Some(tx) = el_tx {
                            let _ = tx.send(translated).await;
                        }
                    }
                    Err(e) => {
                        let msg = format!("DeepL: {e}");
                        log::error!("{msg}");
                        let _ = event_tx.send(TrackEvent::Error {
                            track: track_id,
                            message: msg,
                        }).await;
                    }
                }
            } else {
                // No translation configured — show source as subtitle directly.
                log.log_translated(log_track, "SRC", &text);
                let _ = event_tx.send(TrackEvent::Translated {
                    track: track_id,
                    source: text.clone(),
                    translated: text,
                }).await;
            }
        }

        PipelineEvent::Translated { source_text, translated, .. } => {
            // Emitted if translation was wired inside the pipeline itself
            // (not used in our current setup, but forward it anyway).
            let _ = event_tx.send(TrackEvent::Translated {
                track: track_id,
                source: source_text,
                translated,
            }).await;
        }

        PipelineEvent::Error { error, .. } => {
            log::error!("Pipeline error on track {:?}: {error}", track_id);
            let _ = event_tx.send(TrackEvent::Error {
                track: track_id,
                message: error,
            }).await;
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn resolve_capture_target(source: &TrackSource) -> CaptureTarget {
    match source {
        TrackSource::Mic(None)             => CaptureTarget::Default,
        TrackSource::Mic(Some(id))         => CaptureTarget::Node(*id),
        TrackSource::SinkMonitor(None)     => CaptureTarget::DefaultSinkMonitor,
        TrackSource::SinkMonitor(Some(id)) => CaptureTarget::SinkMonitor(*id),
    }
}
