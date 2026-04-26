//! pipeline — STT / Translate / TTS.
//!
//! Stage 4: streaming STT (Deepgram) + transcript-buffer heuristics.
//! Translation and TTS land in Stages 5–6.
//!
//! The translator-buffer-then-translate architecture is the heart of this
//! crate (DESIGN §4.4). Deepgram's `is_final` doesn't map cleanly to
//! "translate now" — the buffer accumulates finals across thinking pauses
//! and flushes on punctuation / length / silence / UtteranceEnd / manual.

mod deepgram;
mod deepl;
mod events;
mod resample;
mod transcript;

pub use deepgram::{DeepgramClient, DeepgramConfig, DeepgramHandle};
pub use deepl::{DeepLClient, DeepLConfig, TranslationContext};
pub use events::{PipelineEvent, TrackId};
pub use resample::{resample_to_deepgram, ResampleState};
pub use transcript::{FlushReason, TranscriptBuffer, TranscriptBufferConfig};

use std::sync::Once;

use thiserror::Error;

/// Install the `ring` rustls CryptoProvider exactly once. Required
/// before any TLS connection is attempted from this crate; rustls 0.23
/// no longer picks a default and panics inside the WS handshake
/// otherwise. Idempotent and safe to call from multiple binaries.
pub fn ensure_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // `install_default` returns Err if a provider is already set —
        // harmless, ignore.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("deepgram api error: {0}")]
    Deepgram(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("resampler error: {0}")]
    Resample(String),
    #[error("client task ended unexpectedly")]
    ClientGone,
}
