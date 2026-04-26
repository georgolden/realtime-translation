//! Deepgram streaming-STT client.
//!
//! One persistent WebSocket connection per track. Audio in (i16 LE @
//! 16 kHz mono), JSON events out. The client owns the `TranscriptBuffer`
//! and emits `PipelineEvent`s that are already filtered/aggregated.
//!
//! See:
//! - DESIGN.md §4.5
//! - realtime_translator_spec.md → "Deepgram Streaming STT"
//! - https://developers.deepgram.com/docs/live-streaming-audio

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{FutureExt, SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

use crate::events::{PipelineEvent, TrackId};
use crate::transcript::{BufferOutput, TranscriptBuffer, TranscriptBufferConfig};
use crate::PipelineError;

#[derive(Debug, Clone)]
pub struct DeepgramConfig {
    pub api_key:           String,
    pub model:             String,
    /// BCP-47 language code, or `"multi"` for nova-3 multilingual mode
    /// (auto-detects language within the stream). Default is `"multi"`.
    ///
    /// Note: Deepgram's `detect_language=true` param only works for
    /// pre-recorded audio, not streaming. For streaming, `language=multi`
    /// is the correct way to get automatic language detection.
    pub language:          String,
    pub sample_rate:       u32,      // we ship 16_000
    pub channels:          u16,      // we ship 1
    pub interim_results:   bool,     // true
    pub endpointing_ms:    u32,      // 300 (per spec)
    pub utterance_end_ms:  u32,      // 1000 (per spec)
    pub smart_format:      bool,     // true
    pub punctuate:         bool,     // true
    /// Per spec, `endpoint=wss://api.deepgram.com/v1/listen`.
    pub endpoint:          String,
}

impl DeepgramConfig {
    /// Default config: `language=multi` — nova-3 multilingual mode that
    /// auto-detects the language within the stream. This is the correct
    /// streaming equivalent of language detection; `detect_language=true`
    /// is only valid for pre-recorded audio and causes a 400 on streaming.
    ///
    /// **Default values are defined here** — single source of truth used
    /// by both the binary and any future config-file layer.
    pub fn with_detect_language(api_key: String) -> Self {
        Self {
            api_key,
            model:            "nova-3".into(),
            language:         "multi".into(),  // ← nova-3 multilingual auto-detect
            sample_rate:      16_000,
            channels:         1,
            interim_results:  true,
            endpointing_ms:   300,
            utterance_end_ms: 1000,
            smart_format:     true,
            punctuate:        true,
            endpoint:         "wss://api.deepgram.com/v1/listen".into(),
        }
    }

    /// Config with an explicit fixed language code (e.g. "en", "de").
    pub fn with_language(api_key: String, language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            ..Self::with_detect_language(api_key)
        }
    }

    fn build_url(&self) -> String {
        let mut url = self.endpoint.clone();
        url.push('?');

        let pairs: Vec<(&str, String)> = vec![
            ("model",            self.model.clone()),
            ("language",         self.language.clone()),
            ("encoding",         "linear16".into()),
            ("sample_rate",      self.sample_rate.to_string()),
            ("channels",         self.channels.to_string()),
            ("interim_results",  bool_str(self.interim_results).into()),
            ("endpointing",      self.endpointing_ms.to_string()),
            ("utterance_end_ms", self.utterance_end_ms.to_string()),
            ("smart_format",     bool_str(self.smart_format).into()),
            ("punctuate",        bool_str(self.punctuate).into()),
        ];

        url.push_str(
            &pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&"),
        );
        url
    }
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Handle the caller uses to feed audio in + receive events out.
/// Cloneable — dropping the *last* clone closes the audio channel,
/// at which point the WS task sends `CloseStream` and ends.
#[derive(Clone)]
pub struct DeepgramHandle {
    audio_tx: mpsc::Sender<Vec<i16>>,
    /// Set when the audio channel is observed closed, so we only log
    /// the "client task ended" message once instead of on every push.
    closed_logged: Arc<AtomicBool>,
}

impl DeepgramHandle {
    /// Push 16-bit PCM samples (16 kHz mono) into the stream. Non-
    /// blocking: drops on backpressure with a warning. (Deepgram is
    /// faster than realtime in practice; this should never fire.)
    pub fn push_pcm(&self, samples: Vec<i16>) {
        if samples.is_empty() {
            return;
        }
        match self.audio_tx.try_send(samples) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                log::warn!("Deepgram audio queue full — dropping samples");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                if !self.closed_logged.swap(true, Ordering::Relaxed) {
                    log::error!(
                        "Deepgram audio channel closed (client task ended) \
                         — subsequent push_pcm calls will be silently dropped"
                    );
                }
            }
        }
    }

    /// Whether the WS task has ended (audio channel closed). Callers
    /// that pace input may want to stop early instead of pushing into
    /// the void.
    pub fn is_closed(&self) -> bool {
        self.audio_tx.is_closed()
    }
}

pub struct DeepgramClient;

impl DeepgramClient {
    /// Spawn the client. Returns the handle + the event receiver.
    /// Caller can ignore the JoinHandle — the task ends when the
    /// audio channel closes.
    pub fn spawn(
        cfg:        DeepgramConfig,
        buffer_cfg: TranscriptBufferConfig,
        track:      TrackId,
    ) -> (DeepgramHandle, mpsc::Receiver<PipelineEvent>) {
        // rustls 0.23 panics on first TLS handshake unless a crypto
        // provider has been installed. Doing it here covers every
        // caller (binary or library).
        crate::ensure_crypto_provider();

        // 8 frames ≈ 80 ms of latency at 100ms-per-frame Deepgram
        // chunks. Non-blocking try_send drops on full.
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<i16>>(64);
        let (event_tx, event_rx) = mpsc::channel::<PipelineEvent>(256);

        tokio::spawn(async move {
            // Catch panics so a bug inside rustls / tungstenite doesn't
            // silently take down the WS task and leave the audio
            // sender hanging open. Without this, every push_pcm logs an
            // error after the panic.
            let result = AssertUnwindSafe(run_client(
                cfg, buffer_cfg, track, audio_rx, event_tx.clone(),
            ))
            .catch_unwind()
            .await;

            let err_msg = match result {
                Ok(Ok(())) => return,
                Ok(Err(e)) => format!("{e}"),
                Err(panic_payload) => {
                    let s = panic_payload
                        .downcast_ref::<&'static str>()
                        .copied()
                        .or_else(|| {
                            panic_payload
                                .downcast_ref::<String>()
                                .map(String::as_str)
                        })
                        .unwrap_or("<non-string panic>");
                    format!("client task panicked: {s}")
                }
            };
            log::error!("Deepgram client task ended: {err_msg}");
            let _ = event_tx
                .send(PipelineEvent::Error { track, error: err_msg })
                .await;
        });

        (
            DeepgramHandle {
                audio_tx,
                closed_logged: Arc::new(AtomicBool::new(false)),
            },
            event_rx,
        )
    }
}

async fn run_client(
    cfg:        DeepgramConfig,
    buffer_cfg: TranscriptBufferConfig,
    track:      TrackId,
    mut audio_rx: mpsc::Receiver<Vec<i16>>,
    event_tx:   mpsc::Sender<PipelineEvent>,
) -> Result<(), PipelineError> {
    let url = cfg.build_url();
    log::info!("Deepgram: connecting to {}", cfg.endpoint);

    // Build the request manually so we can attach the auth header.
    let mut req = url
        .as_str()
        .into_client_request()
        .map_err(PipelineError::WebSocket)?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Token {}", cfg.api_key))
            .map_err(|_| PipelineError::Deepgram("invalid API key header".into()))?,
    );

    let (ws, _resp) = connect_async(req).await.map_err(PipelineError::WebSocket)?;
    log::info!("Deepgram: connected");

    let (mut ws_sink, mut ws_stream) = ws.split();

    // Buffer + tick task share state. Easiest: keep buffer here, run
    // recv on audio + ws + tick from the same select loop. Single-task,
    // no shared state.
    let mut buf = TranscriptBuffer::new(buffer_cfg);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Outgoing: PCM samples → binary frame.
            maybe_samples = audio_rx.recv() => {
                match maybe_samples {
                    Some(samples) => {
                        let bytes = i16_le_bytes(&samples);
                        if let Err(e) = ws_sink.send(Message::Binary(bytes.into())).await {
                            log::error!("Deepgram: ws send failed: {e}");
                            return Err(PipelineError::WebSocket(e));
                        }
                    }
                    None => {
                        // Audio source finished. Send CloseStream so
                        // Deepgram returns finals before closing.
                        let close = serde_json::json!({ "type": "CloseStream" });
                        let _ = ws_sink.send(Message::Text(close.to_string().into())).await;
                        // Continue draining ws messages until it closes.
                        // We exit the audio branch by setting the channel
                        // to a closed receiver — easiest to set a flag.
                        break;
                    }
                }
            }

            // Incoming: JSON results from Deepgram.
            maybe_msg = ws_stream.next() => {
                match maybe_msg {
                    Some(Ok(msg)) => {
                        if let Err(e) = handle_ws_message(
                            msg,
                            &mut buf,
                            &event_tx,
                            track,
                        ).await {
                            log::warn!("Deepgram: bad ws message: {e}");
                        }
                    }
                    Some(Err(e)) => {
                        log::error!("Deepgram: ws error: {e}");
                        return Err(PipelineError::WebSocket(e));
                    }
                    None => {
                        log::info!("Deepgram: ws closed");
                        break;
                    }
                }
            }

            // Periodic: silence-based flushes.
            _ = tick.tick() => {
                if let Some(out) = buf.on_tick(Instant::now()) {
                    emit_buffer_output(&event_tx, track, out).await;
                }
            }
        }
    }

    // Drain any remaining ws messages after CloseStream so we get the
    // tail finals and any UtteranceEnd.
    while let Some(msg) = ws_stream.next().await {
        match msg {
            Ok(m) => {
                let _ = handle_ws_message(m, &mut buf, &event_tx, track).await;
            }
            Err(_) => break,
        }
    }

    // On EOF, flush whatever's left so the caller's last words don't
    // get silently dropped.
    if let Some(out) = buf.flush_now() {
        emit_buffer_output(&event_tx, track, out).await;
    }

    Ok(())
}

async fn handle_ws_message(
    msg:      Message,
    buf:      &mut TranscriptBuffer,
    event_tx: &mpsc::Sender<PipelineEvent>,
    track:    TrackId,
) -> Result<(), PipelineError> {
    let text = match msg {
        Message::Text(t) => t,
        Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => return Ok(()),
        Message::Close(_) => return Ok(()),
    };

    // Deepgram emits two top-level shapes: transcript Results and
    // metadata. We dispatch on `type`.
    let raw: serde_json::Value = serde_json::from_str(&text)?;
    let kind = raw.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match kind {
        "Results" => {
            let parsed: ResultsMsg = serde_json::from_value(raw)?;
            handle_result(parsed, buf, event_tx, track).await;
        }
        "UtteranceEnd" => {
            log::debug!("Deepgram: UtteranceEnd");
            if let Some(out) = buf.on_utterance_end() {
                emit_buffer_output(event_tx, track, out).await;
            }
        }
        "Metadata" | "SpeechStarted" | "Warning" => {
            log::trace!("Deepgram: {kind}");
        }
        other => {
            log::trace!("Deepgram: unknown message type '{other}'");
        }
    }
    Ok(())
}

async fn handle_result(
    msg:      ResultsMsg,
    buf:      &mut TranscriptBuffer,
    event_tx: &mpsc::Sender<PipelineEvent>,
    track:    TrackId,
) {
    let transcript = msg
        .channel
        .alternatives
        .first()
        .map(|a| a.transcript.as_str())
        .unwrap_or("");
    if transcript.trim().is_empty() {
        return;
    }

    if msg.is_final {
        let outputs = buf.on_final(transcript, Instant::now());
        for o in outputs {
            emit_buffer_output(event_tx, track, o).await;
        }
        // Note: speech_final is an additional hint that endpointing
        // fired. We *could* force a flush here, but DESIGN §4.4 routes
        // all flush triggers through the buffer's own logic for
        // consistency. The endpointing-driven UtteranceEnd will arrive
        // shortly after and trigger the flush.
    } else {
        // is_final=false → partial. Surface to UI; don't mutate buffer.
        let p = buf.on_partial(transcript.to_string()).to_string();
        let _ = event_tx.send(PipelineEvent::Partial { track, text: p }).await;
    }
}

async fn emit_buffer_output(
    event_tx: &mpsc::Sender<PipelineEvent>,
    track:    TrackId,
    out:      BufferOutput,
) {
    let evt = match out {
        BufferOutput::Finalised { text } => PipelineEvent::Finalised { track, text },
        BufferOutput::Flushed { text, reason } => PipelineEvent::Flushed {
            track,
            text,
            reason: reason.as_str(),
        },
    };
    let _ = event_tx.send(evt).await;
}

fn i16_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

// --- Deepgram message schema (just the fields we use). ---
// Full schema is much larger; we tolerate unknown fields with serde's
// default behaviour.

#[derive(Debug, Deserialize)]
struct ResultsMsg {
    #[serde(default)]
    is_final: bool,
    #[serde(default, rename = "speech_final")]
    _speech_final: bool,
    channel: Channel,
}

#[derive(Debug, Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Debug, Deserialize)]
struct Alternative {
    transcript: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── URL-building tests ────────────────────────────────────────────────
    // Default values live in `DeepgramConfig::with_detect_language`.
    // These tests are the machine-readable spec for what lands in the URL.

    // ── URL-building tests ────────────────────────────────────────────────
    // Default values live in `DeepgramConfig::with_detect_language`.
    // These tests are the machine-readable spec for what lands in the URL.

    #[test]
    fn url_auto_detect_uses_multi() {
        // Default (no explicit language) → language=multi in URL.
        // detect_language=true is NOT used — it's only valid for pre-recorded
        // audio, not streaming. nova-3 language=multi is the streaming equivalent.
        let cfg = DeepgramConfig::with_detect_language("dummy".into());
        let url = cfg.build_url();
        assert!(url.starts_with("wss://api.deepgram.com/v1/listen?"));
        assert!(url.contains("language=multi"), "missing language=multi in {url}");
        assert!(!url.contains("detect_language"), "detect_language must not appear in streaming URL: {url}");
        for needle in [
            "model=nova-3",
            "encoding=linear16",
            "sample_rate=16000",
            "channels=1",
            "interim_results=true",
            "endpointing=300",
            "utterance_end_ms=1000",
            "smart_format=true",
            "punctuate=true",
        ] {
            assert!(url.contains(needle), "missing {needle} in {url}");
        }
    }

    #[test]
    fn url_explicit_language_en() {
        let cfg = DeepgramConfig::with_language("dummy".into(), "en");
        let url = cfg.build_url();
        assert!(url.contains("language=en"), "missing language=en in {url}");
        assert!(!url.contains("detect_language"), "detect_language must be absent: {url}");
    }

    #[test]
    fn url_explicit_language_de() {
        let cfg = DeepgramConfig::with_language("dummy".into(), "de");
        let url = cfg.build_url();
        assert!(url.contains("language=de"), "missing language=de in {url}");
        assert!(!url.contains("detect_language"), "detect_language must be absent: {url}");
    }

    #[test]
    fn parse_results_message() {
        // Captured-shape fixture matching what Deepgram actually sends.
        let s = r#"{
            "type": "Results",
            "channel_index": [0,1],
            "duration": 1.0,
            "start": 0.0,
            "is_final": true,
            "speech_final": true,
            "channel": {
                "alternatives": [{
                    "transcript": "hello world",
                    "confidence": 0.99,
                    "words": []
                }]
            }
        }"#;
        let v: serde_json::Value = serde_json::from_str(s).unwrap();
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap();
        assert_eq!(kind, "Results");
        let r: ResultsMsg = serde_json::from_value(v).unwrap();
        assert!(r.is_final);
        assert_eq!(r.channel.alternatives[0].transcript, "hello world");
    }

    #[test]
    fn i16_to_le_round_trip() {
        let samples: Vec<i16> = vec![0, 1, -1, 32767, -32768];
        let bytes = i16_le_bytes(&samples);
        assert_eq!(bytes.len(), samples.len() * 2);
        // First sample is 0x0000.
        assert_eq!(&bytes[0..2], &[0u8, 0u8]);
        // Last sample is 0x8000 little-endian = [0x00, 0x80].
        assert_eq!(&bytes[bytes.len() - 2..], &[0x00, 0x80]);
    }
}
