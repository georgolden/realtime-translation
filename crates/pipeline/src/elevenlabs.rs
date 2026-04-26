//! ElevenLabs WebSocket streaming TTS client — persistent connection.
//!
//! Protocol: single WebSocket connection for the lifetime of the task.
//!   1. Connect wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input
//!              ?model_id=eleven_flash_v2_5&output_format=pcm_24000
//!      Header: xi-api-key: <key>
//!   2. Send init:  {"text":" ","voice_settings":{...},"generation_config":{...}}
//!   3. Per utterance: {"text":"Hello world ","flush":true}
//!   4. Stream audio chunks back as they arrive — forward immediately.
//!   5. Keep connection alive with a space message if idle > ~10s.
//!   6. Drop the text Sender to shut down; task sends "" to flush remaining audio.

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

/// Configuration for the ElevenLabs TTS client.
#[derive(Debug, Clone)]
pub struct ElevenLabsConfig {
    pub api_key:       String,
    pub voice_id:      String,
    pub model_id:      String,
    pub output_format: String,
    pub stability:     f32,
    pub similarity:    f32,
    pub endpoint:      String,
}

impl ElevenLabsConfig {
    pub fn new(api_key: String, voice_id: String) -> Self {
        Self {
            api_key,
            voice_id,
            model_id:      "eleven_flash_v2_5".into(),
            output_format: "pcm_24000".into(),
            stability:     0.5,
            similarity:    0.8,
            endpoint:      "wss://api.elevenlabs.io/v1/text-to-speech".into(),
        }
    }

    fn build_url(&self) -> String {
        format!(
            "{}/{}/stream-input?model_id={}&output_format={}",
            self.endpoint, self.voice_id, self.model_id, self.output_format,
        )
    }

    /// Sample rate implied by `output_format` (e.g. `pcm_24000` → 24000).
    pub fn sample_rate(&self) -> u32 {
        self.output_format
            .strip_prefix("pcm_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(24_000)
    }
}

/// Spawn an ElevenLabs TTS task with a single persistent WebSocket connection.
///
/// Send one translated sentence per message on the returned `Sender`.
/// PCM chunks arrive on the `Receiver` as they stream from EL.
/// `Some(samples)` = audio chunk; `None` = utterance boundary.
/// Drop the `Sender` to gracefully shut down.
pub fn elevenlabs_spawn(
    cfg: ElevenLabsConfig,
) -> (mpsc::Sender<String>, mpsc::Receiver<Option<Vec<f32>>>) {
    crate::ensure_crypto_provider();

    let (text_tx, text_rx) = mpsc::channel::<String>(64);
    let (pcm_tx, pcm_rx)   = mpsc::channel::<Option<Vec<f32>>>(256);

    tokio::spawn(async move {
        if let Err(e) = run_persistent_connection(cfg, text_rx, pcm_tx).await {
            log::error!("ElevenLabs: fatal error: {e}");
        }
        log::debug!("ElevenLabs: task ended");
    });

    (text_tx, pcm_rx)
}

async fn run_persistent_connection(
    cfg:     ElevenLabsConfig,
    mut text_rx: mpsc::Receiver<String>,
    pcm_tx:  mpsc::Sender<Option<Vec<f32>>>,
) -> anyhow::Result<()> {
    let url = cfg.build_url();
    log::info!("ElevenLabs: connecting (persistent) to {}", url);

    let mut req = url.as_str().into_client_request()?;
    req.headers_mut().insert("xi-api-key", HeaderValue::from_str(&cfg.api_key)?);

    let (ws, _) = connect_async(req).await?;
    log::info!("ElevenLabs: connected");

    let (mut ws_sink, mut ws_stream) = ws.split();

    // Init message — must be first.
    let init = serde_json::json!({
        "text": " ",
        "voice_settings": {
            "stability":        cfg.stability,
            "similarity_boost": cfg.similarity,
            "use_speaker_boost": false
        },
        "generation_config": {
            "chunk_length_schedule": [50, 120, 160, 250]
        }
    });
    ws_sink.send(Message::Text(init.to_string().into())).await?;

    // Keepalive: send a space every 10s while idle to prevent server timeout.
    let keepalive_msg = Message::Text(
        serde_json::json!({"text": " "}).to_string().into()
    );
    let mut keepalive = interval(Duration::from_secs(10));
    keepalive.tick().await; // consume the immediate first tick

    let mut last_text_at = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Incoming text from the translation pipeline.
            maybe_text = text_rx.recv() => {
                match maybe_text {
                    Some(text) if !text.trim().is_empty() => {
                        let text_spaced = if text.ends_with(' ') {
                            text.clone()
                        } else {
                            format!("{text} ")
                        };
                        let msg = serde_json::json!({"text": text_spaced, "flush": true});
                        log::info!("ElevenLabs: sending utterance ({} chars)", text.len());
                        ws_sink.send(Message::Text(msg.to_string().into())).await?;
                        last_text_at = tokio::time::Instant::now();
                    }
                    Some(_) => {} // empty string, skip
                    None => {
                        // Sender dropped — flush remaining buffered text and close.
                        log::info!("ElevenLabs: text channel closed, sending close");
                        let close = serde_json::json!({"text": ""});
                        let _ = ws_sink.send(Message::Text(close.to_string().into())).await;
                        break;
                    }
                }
            }

            // Incoming audio from ElevenLabs.
            maybe_msg = ws_stream.next() => {
                match maybe_msg {
                    Some(Ok(Message::Text(t))) => {
                        let v: serde_json::Value = match serde_json::from_str(&t) {
                            Ok(v)  => v,
                            Err(_) => continue,
                        };

                        if let Some(b64) = v.get("audio").and_then(|a| a.as_str()) {
                            if let Some(samples) = decode_pcm(b64) {
                                let _ = pcm_tx.send(Some(samples)).await;
                            }
                        }

                        let is_final = v.get("isFinal").and_then(|f| f.as_bool()).unwrap_or(false);
                        if is_final {
                            // Signal utterance boundary to the playback side.
                            let _ = pcm_tx.send(None).await;
                            log::debug!("ElevenLabs: isFinal received");
                        }
                    }
                    Some(Ok(Message::Binary(b))) => {
                        if !b.is_empty() {
                            let samples: Vec<f32> = b.chunks_exact(2)
                                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                                .collect();
                            let _ = pcm_tx.send(Some(samples)).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        log::info!("ElevenLabs: WS closed by server");
                        break;
                    }
                    Some(Err(e)) => {
                        log::error!("ElevenLabs: WS error: {e}");
                        break;
                    }
                    Some(Ok(_)) => {}
                }
            }

            // Keepalive tick — only send if we've been idle.
            _ = keepalive.tick() => {
                if last_text_at.elapsed() >= Duration::from_secs(8) {
                    log::debug!("ElevenLabs: sending keepalive space");
                    let _ = ws_sink.send(keepalive_msg.clone()).await;
                }
            }
        }
    }

    // Drain any remaining audio after close message.
    while let Ok(Some(Ok(Message::Text(t)))) =
        tokio::time::timeout(Duration::from_secs(2), ws_stream.next()).await
    {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
        if let Some(b64) = v.get("audio").and_then(|a| a.as_str()) {
            if let Some(samples) = decode_pcm(b64) {
                let _ = pcm_tx.send(Some(samples)).await;
            }
        }
    }

    Ok(())
}

fn decode_pcm(b64: &str) -> Option<Vec<f32>> {
    if b64.is_empty() { return None; }
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if bytes.len() < 2 { return None; }
    Some(
        bytes.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_rate_parsed_from_format() {
        let cfg = ElevenLabsConfig::new("key".into(), "vid".into());
        assert_eq!(cfg.sample_rate(), 24_000);
    }

    #[test]
    fn url_contains_voice_and_model() {
        let cfg = ElevenLabsConfig::new("key".into(), "my_voice_id".into());
        let url = cfg.build_url();
        assert!(url.contains("my_voice_id"), "{url}");
        assert!(url.contains("eleven_flash_v2_5"), "{url}");
        assert!(url.contains("pcm_24000"), "{url}");
    }

    #[test]
    fn decode_pcm_valid() {
        let raw = vec![0u8, 0, 0xFF, 0x7F];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        let samples = decode_pcm(&b64).unwrap();
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.0).abs() < 0.001);
        assert!((samples[1] - 1.0).abs() < 0.001);
    }

    #[test]
    fn decode_pcm_empty() {
        assert!(decode_pcm("").is_none());
    }
}
