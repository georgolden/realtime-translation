//! pipeline-stt-stdin — stream audio into Deepgram and print transcript
//! events. Stage 4 manual-testing harness.
//!
//! Usage:
//!   pipeline-stt-stdin --wav FILE [--language CODE]
//!   pipeline-stt-stdin --mic       [--secs N]  [--language CODE]
//!
//! Language detection:
//!   Default (no --language flag): Deepgram auto-detects the language.
//!   With --language de (or any BCP-47 code): fixed language, no detection.
//!   Note: --detect-language is not a flag; detection is the default.
//!
//! Examples:
//!   cargo run --bin pipeline-stt-stdin -- --mic --secs 20
//!   cargo run --bin pipeline-stt-stdin -- --mic --secs 20 --language de
//!   cargo run --bin pipeline-stt-stdin -- --wav /tmp/cap-mic.wav
//!
//! Reads `DEEPGRAM_API_KEY` from `.env` (project root) or the environment.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use audio_os::{capture_for_duration, AudioFormat, CaptureTarget};
use pipeline::{
    DeepgramClient, DeepgramConfig, PipelineEvent, ResampleState,
    TrackId, TranscriptBufferConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let _ = dotenvy::dotenv();

    let opts = parse_args()?;

    let api_key = env::var("DEEPGRAM_API_KEY")
        .map_err(|_| anyhow::anyhow!("DEEPGRAM_API_KEY not set (check .env)"))?;

    let dg_cfg = match opts.language {
        Some(ref lang) => DeepgramConfig::with_language(api_key, lang.as_str()),
        None           => DeepgramConfig::with_detect_language(api_key),
    };
    let buf_cfg = TranscriptBufferConfig::default();
    // "multi" is the internal sentinel for multilingual auto-detect.
    let lang_display = if dg_cfg.language == "multi" { "auto-detect (multi)" } else { &dg_cfg.language };
    log::info!(
        "Deepgram model={} language={}; buffer punct>={} max>={} silence={}ms",
        dg_cfg.model,
        lang_display,
        buf_cfg.min_chars_for_punct_flush,
        buf_cfg.max_chars_before_flush,
        buf_cfg.silence_flush.as_millis(),
    );

    let (handle, mut events) =
        DeepgramClient::spawn(dg_cfg, buf_cfg, TrackId::Outgoing);

    // Print events as they arrive in the foreground task.
    let printer = tokio::spawn(async move {
        let t0 = std::time::Instant::now();
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
                }
                PipelineEvent::Error { error, .. } => {
                    eprintln!("[{dt_ms:>6} ms] ERROR      {error}");
                }
            }
        }
    });

    // Feed audio to the handle on a background task that's source-
    // specific (wav vs. mic).
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
    mode:     Mode,
    /// `None`  → auto-detect language (default, no --language flag given).
    /// `Some`  → fixed BCP-47 code passed via --language; detection disabled.
    language: Option<String>,
}

fn parse_args() -> anyhow::Result<Opts> {
    let mut args = env::args().skip(1);
    let mut mode: Option<Mode> = None;
    // Default: None = auto-detect. Set to Some when --language is given.
    let mut language: Option<String> = None;
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
                // Providing --language explicitly disables auto-detection.
                // There is no --detect-language flag; detection is the default.
                language = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--language needs a BCP-47 code (e.g. en, de, nl)"))?
                );
            }
            "--detect-language" => {
                anyhow::bail!(
                    "--detect-language is not a flag; language auto-detection is the \
                     default. Omit --language to enable it."
                );
            }
            "-h" | "--help" => {
                eprintln!(concat!(
                    "usage: pipeline-stt-stdin [--wav FILE | --mic] [--secs N] [--language CODE]\n",
                    "\n",
                    "Language: omit --language to auto-detect (default).\n",
                    "          --language de  fixes the language and disables detection.",
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
    Ok(Opts { mode, language })
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

    // Convert to interleaved f32 in-memory. Wav files we record in
    // pw-capture-wav are 32-bit float; allow 16-bit int too.
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

    // Resample/feed in 20 ms chunks paced at real-time, so Deepgram's
    // endpointing fires the way it would on a live stream.
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

    // PipeWire's capture loop is sync — run it on a dedicated thread.
    // Push 16 kHz i16 mono into Deepgram via a clone of the handle.
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
