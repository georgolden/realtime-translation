//! pipeline-stt-stdin — stream audio into Deepgram and print transcript +
//! translation events. Stage 4 + Stage 5 manual-testing harness.
//!
//! Usage:
//!   pipeline-stt-stdin --wav FILE [--language CODE] [--source-lang CODE] [--target-lang CODE]
//!   pipeline-stt-stdin --mic       [--secs N]  [--language CODE] [--source-lang CODE] [--target-lang CODE]
//!
//! Deepgram language (STT):
//!   Default (no --language): Deepgram auto-detects the language.
//!   --language de: fix STT language, disables auto-detect.
//!
//! DeepL translation:
//!   Requires DEEPL_API_KEY in .env or environment.
//!   --source-lang: pin DeepL source language (default: auto-detect).
//!   --target-lang: DeepL target language (default: EN).
//!   Omit DEEPL_API_KEY to skip translation and run Stage 4 only.
//!
//! Examples:
//!   cargo run --bin pipeline-stt-stdin -- --mic --secs 20
//!   cargo run --bin pipeline-stt-stdin -- --mic --secs 20 --target-lang DE
//!   cargo run --bin pipeline-stt-stdin -- --wav /tmp/cap-mic.wav --target-lang NL
//!   cargo run --bin pipeline-stt-stdin -- --wav /tmp/cap-mic.wav --source-lang DE --target-lang EN
//!
//! Reads `DEEPGRAM_API_KEY` and `DEEPL_API_KEY` from `.env` (project root)
//! or the environment.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use audio_os::{capture_for_duration, AudioFormat, CaptureTarget};
use pipeline::{
    DeepgramClient, DeepgramConfig, DeepLClient, DeepLConfig, PipelineEvent,
    ResampleState, TrackId, TranscriptBufferConfig, TranslationContext,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let _ = dotenvy::dotenv();

    let opts = parse_args()?;

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
        dg_cfg.model,
        lang_display,
        buf_cfg.min_chars_for_punct_flush,
        buf_cfg.max_chars_before_flush,
        buf_cfg.silence_flush.as_millis(),
    );

    // Stage 5: DeepL is optional — skip if no key is set.
    let deepl: Option<(DeepLClient, usize)> = env::var("DEEPL_API_KEY").ok().map(|key| {
        let mut cfg = DeepLConfig::new(key, opts.source_lang.as_deref(), &opts.target_lang);
        cfg.context_sentences = opts.context_sentences;
        let src_display = cfg.source_lang.as_deref().unwrap_or("auto");
        log::info!(
            "DeepL {} → {} ({}); context window = {} sentences",
            src_display, cfg.target_lang, cfg.model_type, cfg.context_sentences,
        );
        let ctx_size = cfg.context_sentences;
        (DeepLClient::new(cfg), ctx_size)
    });

    if deepl.is_none() {
        log::info!("DEEPL_API_KEY not set — translation disabled (Stage 4 mode)");
    }

    let (handle, mut events) =
        DeepgramClient::spawn(dg_cfg, buf_cfg, TrackId::Outgoing);

    // Print events as they arrive in the foreground task.
    let target_lang = opts.target_lang.clone();
    let printer = tokio::spawn(async move {
        let t0 = std::time::Instant::now();
        let ctx_capacity = deepl.as_ref().map(|(_, n)| *n).unwrap_or(4);
        let mut ctx = TranslationContext::new(ctx_capacity);

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
                                    target_lang.to_uppercase(),
                                    translated,
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "[{:>6} ms] DEEPL ERR  {e}",
                                    t0.elapsed().as_millis()
                                );
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

    // Feed audio to the handle on a background task that's source-specific.
    match opts.mode {
        Mode::Wav(path)  => feed_from_wav(&path, &handle).await?,
        Mode::Mic(secs)  => feed_from_mic(secs, &handle).await?,
    }

    // Drop the handle so the background WS task observes channel-close,
    // sends `CloseStream`, drains finals, and the event channel ends.
    drop(handle);
    let _ = printer.await;
    Ok(())
}

// ============================================================
// Argument parsing.
// ============================================================

#[derive(Debug)]
enum Mode {
    Wav(PathBuf),
    Mic(f32),
}

#[derive(Debug)]
struct Opts {
    mode:             Mode,
    /// Deepgram STT language. `None` → auto-detect (multi).
    language:         Option<String>,
    /// DeepL source language. `None` → DeepL auto-detects.
    source_lang:      Option<String>,
    /// DeepL target language (uppercase). Default: "EN".
    target_lang:      String,
    /// Context window size in sentences. Default: 5.
    context_sentences: usize,
}

fn parse_args() -> anyhow::Result<Opts> {
    let mut args = env::args().skip(1);
    let mut mode: Option<Mode> = None;
    let mut language: Option<String> = None;
    let mut source_lang: Option<String> = None;
    let mut target_lang = "EN".to_string();
    let mut context_sentences: usize = 5;
    let mut mic_secs = 30.0f32;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--wav" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--wav needs a file path"))?;
                mode = Some(Mode::Wav(PathBuf::from(path)));
            }
            "--mic" => {
                if mode.is_none() {
                    mode = Some(Mode::Mic(mic_secs));
                }
            }
            "--secs" => {
                mic_secs = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--secs needs a number"))?
                    .parse()?;
                if let Some(Mode::Mic(_)) = &mode {
                    mode = Some(Mode::Mic(mic_secs));
                }
            }
            "--language" => {
                language = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--language needs a BCP-47 code (e.g. en, de, nl)"))?
                );
            }
            "--source-lang" => {
                source_lang = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--source-lang needs a code (e.g. DE, EN)"))?
                        .to_uppercase()
                );
            }
            "--target-lang" => {
                target_lang = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--target-lang needs a code (e.g. EN, DE, NL)"))?
                    .to_uppercase();
            }
            "--context" => {
                context_sentences = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--context needs a number of sentences"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--context must be a positive integer"))?;
            }
            "--detect-language" => {
                anyhow::bail!(
                    "--detect-language is not a flag; language auto-detection is the \
                     default. Omit --language to enable it."
                );
            }
            "-h" | "--help" => {
                eprintln!(concat!(
                    "usage: pipeline-stt-stdin [--wav FILE | --mic] [--secs N]\n",
                    "                          [--language CODE] [--source-lang CODE]\n",
                    "                          [--target-lang CODE] [--context N]\n",
                    "\n",
                    "STT:          --language de  fixes Deepgram language (default: auto-detect).\n",
                    "Translation:  requires DEEPL_API_KEY in .env or environment.\n",
                    "              --source-lang DE  pin DeepL source (default: auto-detect).\n",
                    "              --target-lang EN  DeepL target language (default: EN).\n",
                    "              --context 5       context window in sentences (default: 5).\n",
                    "              Omit DEEPL_API_KEY to run Stage 4 (STT only).",
                ));
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown flag: {other}"),
        }
    }

    let mode = mode.ok_or_else(|| anyhow::anyhow!("specify --wav FILE or --mic"))?;
    let mode = match mode {
        Mode::Mic(_) => Mode::Mic(mic_secs),
        m => m,
    };
    Ok(Opts { mode, language, source_lang, target_lang, context_sentences })
}

// ============================================================
// WAV → Deepgram, paced at realtime.
// ============================================================

async fn feed_from_wav(
    path:   &std::path::Path,
    handle: &pipeline::DeepgramHandle,
) -> anyhow::Result<()> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    log::info!(
        "wav: {} Hz, {} ch, {} bits, {} samples",
        spec.sample_rate,
        spec.channels,
        spec.bits_per_sample,
        reader.len(),
    );

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
        hound::SampleFormat::Int => {
            let max = (1u32 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / max)
                .collect()
        }
    };

    let in_rate = spec.sample_rate;
    let in_ch   = spec.channels;
    let mut rs = ResampleState::new(in_rate, in_ch)?;

    let chunk_frames = (in_rate as usize / 50) * in_ch as usize; // 20 ms
    let chunk_period = Duration::from_millis(20);
    let mut next_at = tokio::time::Instant::now();
    for chunk in samples_f32.chunks(chunk_frames) {
        if handle.is_closed() {
            log::warn!("wav: Deepgram client closed early — stopping feed");
            break;
        }
        let pcm16 = rs.push(chunk)?;
        if !pcm16.is_empty() {
            handle.push_pcm(pcm16);
        }
        next_at += chunk_period;
        tokio::time::sleep_until(next_at).await;
    }
    log::info!("wav: streaming complete");
    Ok(())
}

// ============================================================
// Live mic → Deepgram. PipeWire capture runs on a blocking thread.
// ============================================================

async fn feed_from_mic(
    secs:   f32,
    handle: &pipeline::DeepgramHandle,
) -> anyhow::Result<()> {
    log::info!("mic: capturing default source for {secs:.1}s — start speaking");

    let handle_for_capture = handle.clone();
    let capture_thread = std::thread::spawn(move || -> anyhow::Result<()> {
        let mut state: Option<ResampleState> = None;
        capture_for_duration(
            CaptureTarget::Default,
            Duration::from_secs_f32(secs),
            move |samples: &[f32], fmt: AudioFormat| {
                let rs = state.get_or_insert_with(|| {
                    log::info!(
                        "mic: building resampler for {} Hz × {} ch",
                        fmt.sample_rate, fmt.channels,
                    );
                    ResampleState::new(fmt.sample_rate, fmt.channels)
                        .expect("resampler init")
                });
                let pcm16 = match rs.push(samples) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("resample error: {e}");
                        return;
                    }
                };
                if !pcm16.is_empty() {
                    handle_for_capture.push_pcm(pcm16);
                }
            },
        )?;
        Ok(())
    });

    tokio::task::spawn_blocking(move || capture_thread.join())
        .await
        .map_err(|e| anyhow::anyhow!("join task panic: {e}"))?
        .map_err(|_| anyhow::anyhow!("capture thread panicked"))??;

    log::info!("mic: capture finished");
    Ok(())
}
