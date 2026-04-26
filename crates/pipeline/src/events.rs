//! Events the pipeline emits to the rest of the app.

/// Identifies which side of the conversation an event belongs to.
/// Stage 4 only uses `Outgoing`; `Incoming` shows up at Stage 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackId {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// Live partial from Deepgram. UI shows it dimmed; not yet committed.
    Partial {
        track: TrackId,
        text:  String,
    },
    /// A `is_final: true` segment landed in the buffer. Not yet flushed.
    /// The UI may use this to render committed-but-untranslated text.
    Finalised {
        track: TrackId,
        text:  String,
    },
    /// The transcript buffer flushed. Source text is ready; translation
    /// will arrive shortly as a `Translated` event.
    Flushed {
        track:   TrackId,
        text:    String,
        reason:  FlushReasonStr,
    },
    /// DeepL returned a translation for a flushed chunk.
    Translated {
        track:        TrackId,
        source_text:  String,
        translated:   String,
    },
    /// A non-fatal error from one of the streaming components. The
    /// pipeline keeps running.
    Error {
        track: TrackId,
        error: String,
    },
}

/// Stringly-typed flush reason for log/debug. The internal state machine
/// uses `transcript::FlushReason`; we lower it to a label here so the
/// event type doesn't leak the internal enum's exhaustiveness.
pub type FlushReasonStr = &'static str;
