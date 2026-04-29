//! TranscriptBuffer — the heuristic layer between Deepgram and DeepL.
//!
//! See DESIGN.md §4.4. The state machine is deliberately small and pure:
//! given a sequence of inputs (final segment, partial, UtteranceEnd, tick,
//! manual flush) it emits a sequence of outputs (Finalised / Flushed
//! events). All thresholds live in `TranscriptBufferConfig` so they're
//! tunable from `config.toml` without touching code.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct TranscriptBufferConfig {
    /// Flush if buffer ends in `.`/`?`/`!`/`…` and at least this many
    /// chars accumulated.
    pub min_chars_for_punct_flush: usize,
    /// Flush after this many chars even without punctuation. Prevents
    /// runaway accumulation on a long monologue.
    pub max_chars_before_flush:    usize,
    /// Flush if no new word arrived for this long. Covers thinking
    /// pauses without losing the in-flight meaning.
    pub silence_flush:             Duration,
    /// Always flush on UtteranceEnd from Deepgram.
    pub flush_on_utterance_end:    bool,
}

impl Default for TranscriptBufferConfig {
    fn default() -> Self {
        Self {
            min_chars_for_punct_flush: 30,
            max_chars_before_flush:    240,
            silence_flush:             Duration::from_millis(500),
            flush_on_utterance_end:    true,
        }
    }
}

/// Why the buffer flushed. Useful for logging and tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    Punctuation,
    MaxChars,
    Silence,
    UtteranceEnd,
    Manual,
}

impl FlushReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FlushReason::Punctuation  => "punctuation",
            FlushReason::MaxChars     => "max-chars",
            FlushReason::Silence      => "silence",
            FlushReason::UtteranceEnd => "utterance-end",
            FlushReason::Manual       => "manual",
        }
    }
}

/// Output of a single buffer step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferOutput {
    /// A final landed and was appended; nothing flushed yet.
    Finalised { text: String },
    /// Flush triggered. `text` is the full accumulated buffer (cleared
    /// after this output).
    Flushed { text: String, reason: FlushReason },
}

/// State machine. Holds the running buffer, the time of the last word,
/// and the latest partial (for UI).
pub struct TranscriptBuffer {
    cfg:           TranscriptBufferConfig,
    buf:           String,
    last_word_at:  Option<Instant>,
    last_partial:  String,
}

impl TranscriptBuffer {
    pub fn new(cfg: TranscriptBufferConfig) -> Self {
        Self {
            cfg,
            buf:          String::new(),
            last_word_at: None,
            last_partial: String::new(),
        }
    }

    pub fn config(&self) -> &TranscriptBufferConfig {
        &self.cfg
    }

    /// Current accumulated text (for inspection / tests).
    pub fn current(&self) -> &str {
        &self.buf
    }

    /// Latest partial Deepgram emitted. Cleared whenever a final lands.
    pub fn latest_partial(&self) -> &str {
        &self.last_partial
    }

    /// Update the latest partial. Returns the new partial as a borrow
    /// (the caller usually surfaces this to the UI).
    pub fn on_partial(&mut self, text: String) -> &str {
        self.last_partial = text;
        &self.last_partial
    }

    /// Append a final segment. Accumulates into buffer; flushes when the
    /// buffer ends with sentence-terminal punctuation above min_chars, or
    /// exceeds max_chars.
    pub fn on_final(&mut self, text: &str, now: Instant) -> Vec<BufferOutput> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        self.last_partial.clear();

        if !self.buf.is_empty() && !self.buf.ends_with(' ') {
            self.buf.push(' ');
        }
        self.buf.push_str(text.trim());
        self.last_word_at = Some(now);

        let mut out = Vec::new();
        if self.should_flush_on_punct() {
            out.push(self.take_flush(FlushReason::Punctuation));
        } else if self.buf.chars().count() >= self.cfg.max_chars_before_flush {
            out.push(self.take_flush(FlushReason::MaxChars));
        }
        out
    }

    fn should_flush_on_punct(&self) -> bool {
        if self.buf.chars().count() < self.cfg.min_chars_for_punct_flush {
            return false;
        }
        matches!(
            self.buf.trim_end().chars().last(),
            Some('.') | Some('?') | Some('!') | Some('…')
        )
    }

    /// Hard end-of-speech signal from Deepgram.
    pub fn on_utterance_end(&mut self) -> Option<BufferOutput> {
        if self.cfg.flush_on_utterance_end && !self.buf.is_empty() {
            Some(self.take_flush(FlushReason::UtteranceEnd))
        } else {
            None
        }
    }

    /// Periodic tick — caller drives this every ~100 ms with the current
    /// time. Triggers a silence flush if `silence_flush` elapsed since
    /// the last word.
    pub fn on_tick(&mut self, now: Instant) -> Option<BufferOutput> {
        let last = self.last_word_at?;
        if self.buf.is_empty() {
            return None;
        }
        if now.duration_since(last) >= self.cfg.silence_flush {
            Some(self.take_flush(FlushReason::Silence))
        } else {
            None
        }
    }

    /// Manual flush — bound to a hotkey / button at the UI layer.
    pub fn flush_now(&mut self) -> Option<BufferOutput> {
        if self.buf.is_empty() {
            None
        } else {
            Some(self.take_flush(FlushReason::Manual))
        }
    }

    fn take_flush(&mut self, reason: FlushReason) -> BufferOutput {
        let text = std::mem::take(&mut self.buf);
        self.last_word_at = None;
        BufferOutput::Flushed { text, reason }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TranscriptBufferConfig {
        TranscriptBufferConfig {
            min_chars_for_punct_flush: 10,
            max_chars_before_flush:    50,
            silence_flush:             Duration::from_millis(500),
            flush_on_utterance_end:    true,
        }
    }

    #[test]
    fn empty_final_is_ignored() {
        let mut b = TranscriptBuffer::new(cfg());
        let out = b.on_final("   ", Instant::now());
        assert!(out.is_empty());
        assert_eq!(b.current(), "");
    }

    #[test]
    fn short_final_stays_buffered() {
        let mut b = TranscriptBuffer::new(cfg());
        // "hi." is 3 chars — below min_chars_for_punct_flush=10, stays in buf.
        let out = b.on_final("hi.", Instant::now());
        assert!(out.is_empty(), "short sentence should stay buffered, got {out:?}");
        assert_eq!(b.current(), "hi.");
    }

    #[test]
    fn punctuation_above_threshold_flushes() {
        let mut b = TranscriptBuffer::new(cfg());
        let out = b.on_final("hello there friend.", Instant::now());
        assert_eq!(out.len(), 1);
        match &out[0] {
            BufferOutput::Flushed { text, reason } => {
                assert_eq!(text, "hello there friend.");
                assert_eq!(*reason, FlushReason::Punctuation);
            }
            other => panic!("expected Flushed, got {other:?}"),
        }
        assert_eq!(b.current(), "");
    }

    #[test]
    fn finals_accumulate_across_pauses() {
        let mut b = TranscriptBuffer::new(cfg());
        // Fragments accumulate in buffer until a punct-terminated chunk arrives.
        b.on_final("um", Instant::now());
        b.on_final("hello there", Instant::now());
        let out = b.on_final("friend.", Instant::now());
        // "um hello there friend." — above min_chars, ends with punct → flush.
        assert!(out.iter().any(|o| matches!(
            o,
            BufferOutput::Flushed { reason: FlushReason::Punctuation, .. }
        )));
    }

    #[test]
    fn max_chars_flush_without_punctuation() {
        let mut b = TranscriptBuffer::new(cfg());
        // Build a long final with no terminal punctuation.
        let big = "word ".repeat(20); // ~100 chars
        let out = b.on_final(big.trim(), Instant::now());
        assert!(out.iter().any(|o| matches!(
            o,
            BufferOutput::Flushed { reason: FlushReason::MaxChars, .. }
        )));
    }

    #[test]
    fn silence_flush_after_threshold() {
        let mut b = TranscriptBuffer::new(cfg());
        let t0 = Instant::now();
        b.on_final("we have something", t0);
        // Tick before silence threshold — no flush.
        assert!(b.on_tick(t0 + Duration::from_millis(200)).is_none());
        // Tick after silence threshold — flush.
        let f = b.on_tick(t0 + Duration::from_millis(600)).expect("flush");
        assert!(matches!(
            f,
            BufferOutput::Flushed { reason: FlushReason::Silence, .. }
        ));
        assert_eq!(b.current(), "");
    }

    #[test]
    fn silence_flush_does_nothing_when_empty() {
        let mut b = TranscriptBuffer::new(cfg());
        assert!(b.on_tick(Instant::now()).is_none());
    }

    #[test]
    fn utterance_end_flushes() {
        let mut b = TranscriptBuffer::new(cfg());
        b.on_final("partial thought", Instant::now());
        let f = b.on_utterance_end().expect("flush");
        assert!(matches!(
            f,
            BufferOutput::Flushed { reason: FlushReason::UtteranceEnd, .. }
        ));
    }

    #[test]
    fn utterance_end_on_empty_is_noop() {
        let mut b = TranscriptBuffer::new(cfg());
        assert!(b.on_utterance_end().is_none());
    }

    #[test]
    fn manual_flush() {
        let mut b = TranscriptBuffer::new(cfg());
        b.on_final("anything", Instant::now());
        let f = b.flush_now().expect("flush");
        assert!(matches!(
            f,
            BufferOutput::Flushed { reason: FlushReason::Manual, .. }
        ));
        assert!(b.flush_now().is_none()); // already empty
    }

    #[test]
    fn partial_does_not_affect_buffer() {
        let mut b = TranscriptBuffer::new(cfg());
        b.on_final("committed", Instant::now());
        b.on_partial("committed and more in flight".into());
        assert_eq!(b.current(), "committed");
        assert_eq!(b.latest_partial(), "committed and more in flight");
    }

    #[test]
    fn final_clears_partial() {
        let mut b = TranscriptBuffer::new(cfg());
        b.on_partial("draft".into());
        b.on_final("real", Instant::now());
        assert_eq!(b.latest_partial(), "");
    }

}
