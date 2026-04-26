//! pipeline-stt-stdin — Stage 4 + 5 + 6 manual-testing harness.
//!
//! Streams mic/wav → Deepgram STT → DeepL translation → ElevenLabs TTS
//! → plays audio to a PipeWire sink (default sink, or virtmic by name).
//!
//! Each stage activates when its env key is present:
//!   Stage 4: DEEPGRAM_API_KEY  — STT only
//!   Stage 5: + DEEPL_API_KEY   — + translation
//!   Stage 6: + ELEVENLABS_API_KEY + VOICE_ID — + TTS playback
//!
//! Usage:
//!   pipeline-stt-stdin --wav FILE
//!   pipeline-stt-stdin --mic [--secs N]
//!
//! Flags:
//!   --language CODE     Deepgram STT language (default: auto-detect)
//!   --source-lang CODE  DeepL source language (default: auto-detect)
//!   --target-lang CODE  DeepL target language (default: EN)
//!   --context N         Context window in sentences (default: 5)
//!   --sink NAME         PipeWire sink node.name for TTS output
//!                       (default: system default sink)
//!
//! Examples:
//!   cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav --target-lang DE
//!   cargo run --bin pipeline-stt-stdin -- --mic --secs 30
//!   cargo run --bin pipeline-stt-stdin -- --mic --secs 30 \
//!       --sink translator_virtmic_sink

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use audio_os::{
    capture_for_duration, AudioFormat, CaptureTarget,
    PlaybackFormat, PlaybackTarget, spawn_streaming_player,
};
use pipeline::{
    elevenlabs_spawn, DeepgramClient, DeepgramConfig, DeepLClient, DeepLConfig,
    ElevenLabsConfig, PipelineEvent, ResampleState, TrackId, TranscriptBufferConfig,
    TranslationContext,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let _ = dotenvy::dotenv();

    let opts = parse_args()?;

    // ── Stage 4: Deepgram ──────────────────────────────────────────────
    let dg_api_key = env::var("DEEPGRAM_API_KEY")
        .map_err(|_| anyhow::anyhow!("DEEPGRAM_API_KEY not set (check .env)"))?;

    let dg_cfg = match opts.language {
        Some(ref lang) => DeepgramConfig::with_language(dg_api_key, lang.as_str()),
        None           => DeepgramConfig::with_detect_language(dg_api_key),
    };
    let buf_cfg = TranscriptBufferConfig::default();
    let lang_display = if dg_cfg.language == "multi" { "auto-detect (multi)" } else { &dg_cfg.language };
    log::info!(
        "Deepgram model={} language={}; buffer punct>={} max>={} silence={}ms",
        dg_cfg.model, lang_display,
        buf_cfg.min_chars_for_punct_flush, buf_cfg.max_chars_before_flush,
        buf_cfg.silence_flush.as_millis(),
    );

    // ── Stage 5: DeepL (optional) ─────────────────────────────────────
    let deepl: Option<(DeepLClient, usize)> = env::var("DEEPL_API_KEY").ok().map(|key| {
        let mut cfg = DeepLConfig::new(key, opts.source_lang.as_deref(), &opts.target_lang);
        cfg.context_sentences = opts.context_sentences;
        let src = cfg.source_lang.as_deref().unwrap_or("auto");
        log::info!("DeepL {} → {} ({}); context={} sentences",
            src, cfg.target_lang, cfg.model_type, cfg.context_sentences);
        let sz = cfg.context_sentences;
        (DeepLClient::new(cfg), sz)
    });
    if deepl.is_none() {
        log::info!("DEEPL_API_KEY not set — translation disabled (Stage 4 only)");
    }

    // ── Stage 6: ElevenLabs + playback (optional) ─────────────────────
    let el_tx: Option<tokio::sync::mpsc::Sender<String>> =
        match (env::var("ELEVENLABS_API_KEY"), env::var("VOICE_ID")) {
            (Ok(api_key), Ok(voice_id)) => {
                let el_cfg = ElevenLabsConfig::new(api_key, voice_id);
                let sample_rate = el_cfg.sample_rate();
                log::info!("ElevenLabs model={} format={} → playback {}Hz",
                    el_cfg.model_id, el_cfg.output_format, sample_rate);

                let (text_tx, mut pcm_rx) = elevenlabs_spawn(el_cfg);

                let pb_target = match opts.sink_name {
                    Some(ref name) => PlaybackTarget::NodeName(name.clone()),
                    None           => PlaybackTarget::Default,
                };
                let pb_format = PlaybackFormat { sample_rate, channels: 1 };

                // Single persistent PipeWire stream for the entire session.
                // Ring: 30 s mono @ 24 kHz — enough to queue several utterances.
                const RING_CAP: usize = 24_000 * 30;

                let (mut pw_handle, pw_join) = spawn_streaming_player(
                    pb_target,
                    pb_format,
                    RING_CAP,
                );
                log::info!("TTS: persistent streaming player opened");

                // Feed all PCM chunks into the ring; ignore utterance boundaries.
                tokio::spawn(async move {
                    while let Some(msg) = pcm_rx.recv().await {
                        if let Some(samples) = msg {
                            pw_handle.push_pcm(&samples);
                        }
                        // None = utterance boundary — no-op, ring acts as queue
                    }
                    // Sender dropped: signal done and wait for drain.
                    pw_handle.finish();
                    log::info!("TTS: playback task ended");
                    let _ = tokio::task::spawn_blocking(move || pw_join.join()).await;
                });

                Some(text_tx)
            }
            _ => {
                log::info!("ELEVENLABS_API_KEY / VOICE_ID not set — TTS disabled (Stage 5 only)");
                None
            }
        };

    let (handle, mut events) = DeepgramClient::spawn(dg_cfg, buf_cfg, TrackId::Outgoing);

    // ── Event printer + translation + TTS dispatch ────────────────────
    let target_lang  = opts.target_lang.clone();
    let printer = tokio::spawn(async move {
        let t0  = std::time::Instant::now();
        let ctx_cap = deepl.as_ref().map(|(_, n)| *n).unwrap_or(5);
        let mut ctx = TranslationContext::new(ctx_cap);

        while let Some(evt) = events.recv().await {
            let dt_ms = t0.elapsed().as_millis();
            match evt {
                PipelineEvent::Partial { text, .. } => {
                    println!("[{dt_ms:>6} ms] PARTIAL    {text}");
                }
                PipelineEvent::Finalised { text, .. } => {
                    println!("[{dt_ms:>6} ms] FINALISED  {text}");
                }
                PipelineEvent::Flushed { text, reason, .. } => {
                    println!("[{dt_ms:>6} ms] FLUSHED    ({reason})  {text}");

                    if let Some((ref dl, _)) = deepl {
                        let context = ctx.push_and_context(&text);
                        match dl.translate(&text, &context).await {
                            Ok(translated) => {
                                println!(
                                    "[{:>6} ms] TRANSLATED [→ {}]  {}",
                                    t0.elapsed().as_millis(),
                                    target_lang,
                                    translated,
                                );
                                // Stage 6: one message per utterance — EL opens a
                                // fresh connection, sends text+flush, drains audio.
                                if let Some(ref tx) = el_tx {
                                    let _ = tx.send(translated).await;
                                }
                            }
                            Err(e) => {
                                eprintln!("[{:>6} ms] DEEPL ERR  {e}", t0.elapsed().as_millis());
                            }
                        }
                    }
                }
                PipelineEvent::Translated { source_text, translated, .. } => {
                    println!("[{dt_ms:>6} ms] TRANSLATED {source_text} → {translated}");
                }
                PipelineEvent::Error { error, .. } => {
                    eprintln!("[{dt_ms:>6} ms] ERROR      {error}");
                }
            }
        }
    });

    match opts.mode {
        Mode::Wav(path) => feed_from_wav(&path, &handle).await?,
        Mode::Mic(secs) => feed_from_mic(secs, &handle).await?,
    }

    drop(handle);
    let _ = printer.await;
    Ok(())
}

// ── Argument parsing ──────────────────────────────────────────────────

#[derive(Debug)]
enum Mode {
    Wav(PathBuf),
    Mic(f32),
}

#[derive(Debug)]
struct Opts {
    mode:              Mode,
    language:          Option<String>,
    source_lang:       Option<String>,
    target_lang:       String,
    context_sentences: usize,
    sink_name:         Option<String>,
}

fn parse_args() -> anyhow::Result<Opts> {
    let mut args = env::args().skip(1);
    let mut mode: Option<Mode> = None;
    let mut language:    Option<String> = None;
    let mut source_lang: Option<String> = None;
    let mut target_lang  = "EN".to_string();
    let mut context_sentences: usize = 5;
    let mut mic_secs = 30.0f32;
    let mut sink_name: Option<String> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--wav" => {
                let path = args.next().ok_or_else(|| anyhow::anyhow!("--wav needs a path"))?;
                mode = Some(Mode::Wav(PathBuf::from(path)));
            }
            "--mic" => { if mode.is_none() { mode = Some(Mode::Mic(mic_secs)); } }
            "--secs" => {
                mic_secs = args.next().ok_or_else(|| anyhow::anyhow!("--secs needs a number"))?.parse()?;
                if let Some(Mode::Mic(_)) = &mode { mode = Some(Mode::Mic(mic_secs)); }
            }
            "--language" => {
                language = Some(args.next().ok_or_else(|| anyhow::anyhow!("--language needs a code"))?);
            }
            "--source-lang" => {
                source_lang = Some(
                    args.next().ok_or_else(|| anyhow::anyhow!("--source-lang needs a code"))?.to_uppercase()
                );
            }
            "--target-lang" => {
                target_lang = args.next().ok_or_else(|| anyhow::anyhow!("--target-lang needs a code"))?.to_uppercase();
            }
            "--context" => {
                context_sentences = args.next()
                    .ok_or_else(|| anyhow::anyhow!("--context needs a number"))?
                    .parse().map_err(|_| anyhow::anyhow!("--context must be a positive integer"))?;
            }
            "--sink" => {
                sink_name = Some(args.next().ok_or_else(|| anyhow::anyhow!("--sink needs a node name"))?);
            }
            "--detect-language" => {
                anyhow::bail!("--detect-language is not a flag; omit --language to auto-detect.");
            }
            "-h" | "--help" => {
                eprintln!(concat!(
                    "usage: pipeline-stt-stdin [--wav FILE | --mic] [--secs N]\n",
                    "                          [--language CODE] [--source-lang CODE]\n",
                    "                          [--target-lang CODE] [--context N]\n",
                    "                          [--sink NODE_NAME]\n",
                    "\n",
                    "Env keys required per stage:\n",
                    "  Stage 4 (STT):           DEEPGRAM_API_KEY\n",
                    "  Stage 5 (+translation):  + DEEPL_API_KEY\n",
                    "  Stage 6 (+TTS):          + ELEVENLABS_API_KEY + VOICE_ID\n",
                ));
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown flag: {other}"),
        }
    }

    let mode = mode.ok_or_else(|| anyhow::anyhow!("specify --wav FILE or --mic"))?;
    let mode = match mode { Mode::Mic(_) => Mode::Mic(mic_secs), m => m };
    Ok(Opts { mode, language, source_lang, target_lang, context_sentences, sink_name })
}

// ── Audio feeding ─────────────────────────────────────────────────────

async fn feed_from_wav(path: &std::path::Path, handle: &pipeline::DeepgramHandle) -> anyhow::Result<()> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    log::info!("wav: {} Hz, {} ch, {} bits, {} samples",
        spec.sample_rate, spec.channels, spec.bits_per_sample, reader.len());

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let max = (1u32 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap_or(0) as f32 / max).collect()
        }
    };

    let mut rs = ResampleState::new(spec.sample_rate, spec.channels)?;
    let chunk_frames = (spec.sample_rate as usize / 50) * spec.channels as usize;
    let chunk_period = Duration::from_millis(20);
    let mut next_at  = tokio::time::Instant::now();

    for chunk in samples_f32.chunks(chunk_frames) {
        if handle.is_closed() {
            log::warn!("wav: Deepgram closed early — stopping");
            break;
        }
        let pcm16 = rs.push(chunk)?;
        if !pcm16.is_empty() { handle.push_pcm(pcm16); }
        next_at += chunk_period;
        tokio::time::sleep_until(next_at).await;
    }
    log::info!("wav: streaming complete");
    Ok(())
}

async fn feed_from_mic(secs: f32, handle: &pipeline::DeepgramHandle) -> anyhow::Result<()> {
    log::info!("mic: capturing default source for {secs:.1}s — start speaking");
    let h = handle.clone();
    let t = std::thread::spawn(move || -> anyhow::Result<()> {
        let mut state: Option<ResampleState> = None;
        capture_for_duration(
            CaptureTarget::Default,
            Duration::from_secs_f32(secs),
            move |samples: &[f32], fmt: AudioFormat| {
                let rs = state.get_or_insert_with(|| {
                    log::info!("mic: resampler {} Hz × {} ch", fmt.sample_rate, fmt.channels);
                    ResampleState::new(fmt.sample_rate, fmt.channels).expect("resampler init")
                });
                if let Ok(pcm16) = rs.push(samples) {
                    if !pcm16.is_empty() { h.push_pcm(pcm16); }
                }
            },
        )?;
        Ok(())
    });
    tokio::task::spawn_blocking(move || t.join())
        .await
        .map_err(|e| anyhow::anyhow!("join panic: {e}"))?
        .map_err(|_| anyhow::anyhow!("capture thread panicked"))??;
    log::info!("mic: capture finished");
    Ok(())
}
