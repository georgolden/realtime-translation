# UI Rewrite Plan

**Status:** Active plan — work in progress  
**Date:** 2026-04-26

---

## Problems with the current UI crate (to fix)

1. **Panic: Duration too long** — `capture_for_duration` called with `Duration::from_secs(u64::MAX)`.
   PipeWire's timer API cannot represent this value (`TryFromIntError`). Fix: replace
   `capture_for_duration` with a new `capture_indefinite` function that runs until a
   stop flag is set (no timer arm at all).

2. **Deepgram closes after ~12s** — because the mic capture thread panics on the bad duration,
   the audio channel drops, `CloseStream` is sent, and the WS closes. Everything downstream dies.

3. **No Track 2 (incoming audio)** — the second parallel pipeline (browser/system audio → STT
   → subtitles) is entirely missing. The design requires two simultaneous Deepgram streams.

4. **No session transcript logging** — all transcribed + translated text must be written to a
   timestamped file on disk as the session runs, so nothing is lost.

5. **UI is one 430-line blob** — no structure; mixes session runner logic, state, rendering.

6. **Transparent overlay fails on XFCE/X11** — eframe logs "Cannot create transparent window:
   the GL config does not support it". Use opaque dark background fallback instead.

7. **No config.toml support** — language, buffer thresholds, sink names should load from
   `~/.config/realtime-translation/config.toml` on startup, not just from `.env`.

---

## Two-pipeline architecture

```
┌─────────────────────────────────────────────────┐
│  Track 1 — OUTGOING (mandatory)                 │
│                                                 │
│  Mic capture (selected node or default)         │
│    → resample 48k f32 → 16k i16 mono            │
│    → Deepgram WS (nova-3, language=multi)       │
│    → TranscriptBuffer                           │
│    → DeepL HTTP  (optional — needs DEEPL key)   │
│    → ElevenLabs WS  (optional — needs EL+VOICE) │
│    → PipeWire streaming player                  │
│       → translator_virtmic_sink (default)       │
│       → or any user-chosen sink                 │
│    → session transcript log                     │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│  Track 2 — INCOMING (optional — no keys needed) │
│                                                 │
│  System/browser audio capture                   │
│  (SinkMonitor of selected sink or default sink) │
│    → resample 48k f32 → 16k i16 mono            │
│    → Deepgram WS (nova-3, language=multi)       │
│    → TranscriptBuffer                           │
│    → DeepL HTTP  (optional — same key as T1)    │
│    → subtitle overlay only — NO TTS             │
│    → session transcript log                     │
└─────────────────────────────────────────────────┘
```

Both tracks run in parallel from session start to session stop. Either track can fail without
taking down the other (errors surface in UI but do not panic).

---

## File structure after rewrite

```
crates/ui/src/
├── lib.rs          — public exports only (re-exports, top-level doc)
├── config.rs       — AppConfig: load/save config.toml + .env; all tunable params
├── session.rs      — SessionHandle: spawns both tracks, owns channels, lifetime
├── track.rs        — single-track runner: mic or sink-monitor → STT → translate → (TTS)
├── transcript.rs   — TranscriptLog: timestamped append-only log file writer
├── state.rs        — UiState: subtitle history, partial text, session status
└── app.rs          — TranslatorApp: eframe App impl; control window + overlay rendering
```

---

## audio-os additions needed

### `capture_indefinite`

The existing `capture_for_duration` uses a PipeWire timer to stop after a fixed duration.
We need a version that runs until a `stop` flag is set:

```rust
pub fn capture_indefinite<F>(
    target: CaptureTarget,
    stop: Arc<AtomicBool>,
    on_frame: F,
) -> Result<(), AudioOsError>
where
    F: FnMut(&[f32], AudioFormat) + 'static
```

Internally: add a 50ms polling timer that checks `stop.load()` and calls `mainloop.quit()`.
No `u64::MAX` anywhere.

---

## SessionConfig fields (after rewrite)

```rust
pub struct SessionConfig {
    // Track 1 — always active
    pub mic_node_id:      Option<u32>,    // None = PW default source
    pub source_lang:      Option<String>, // None = Deepgram auto-detect (multi)
    pub target_lang:      String,         // DeepL / display target

    // Track 1 TTS output
    pub tts_sink_name:    Option<String>, // None = default sink (headphones)
    //   Set to "translator_virtmic_sink" to route to meeting

    // Track 2 — optional; enabled when track2_enabled = true
    pub track2_enabled:   bool,
    pub sink_node_id:     Option<u32>,    // None = default sink monitor

    // API keys
    pub dg_api_key:       String,         // required
    pub deepl_api_key:    Option<String>, // optional
    pub el_api_key:       Option<String>, // optional (needs voice_id too)
    pub voice_id:         Option<String>, // optional

    // Buffer tuning
    pub buf_cfg:          TranscriptBufferConfig,
    pub context_sentences: usize,
}
```

---

## TranscriptLog format

File: `~/.local/share/realtime-translation/sessions/YYYY-MM-DD_HH-MM-SS.log`

```
[2026-04-26T15:30:01Z] [MIC]    PARTIAL   hello there
[2026-04-26T15:30:02Z] [MIC]    SOURCE    hello there friend.
[2026-04-26T15:30:02Z] [MIC]    DE        Hallo da, Freund.
[2026-04-26T15:30:05Z] [AUDIO]  PARTIAL   gut morgen
[2026-04-26T15:30:06Z] [AUDIO]  SOURCE    guten Morgen, wie geht es Ihnen?
[2026-04-26T15:30:07Z] [AUDIO]  EN        Good morning, how are you?
```

Format: `[ISO8601] [TRACK] TYPE  text`
- TRACK: `MIC` = Track 1 outgoing, `AUDIO` = Track 2 incoming
- TYPE: `PARTIAL`, `SOURCE` (flushed source text), `<LANG_CODE>` (translated)

---

## UI layout

### Control window (620 × 680 px)

```
┌────────────────────────────────────────────┐
│ Realtime Translator                        │
├────────────────────────────────────────────┤
│ Audio Sources                              │
│   Microphone:      [dropdown]  [Refresh]   │
│   Source language: [dropdown / auto]       │
│   Target language: [dropdown]              │
├────────────────────────────────────────────┤
│ Track 2 — Incoming subtitles  [checkbox]   │
│   Audio source:    [dropdown]              │
├────────────────────────────────────────────┤
│ TTS Output                                 │
│   Route to sink:   [text field]            │
│       translator_virtmic_sink (default)    │
├────────────────────────────────────────────┤
│ ▼ API Keys (collapsible)                   │
│   Deepgram:    [password]  required        │
│   DeepL:       [password]  optional        │
│   ElevenLabs:  [password]  optional        │
│   Voice ID:    [text]      optional        │
├────────────────────────────────────────────┤
│ ▼ Advanced (collapsible)                   │
│   Silence flush (ms): [400]                │
│   Min chars punct:    [30]                 │
│   Max chars:          [240]                │
│   Context sentences:  [5]                  │
├────────────────────────────────────────────┤
│ [▶ Start]                                  │
│  — or when running —                       │
│ [■ Stop]  [Flush now]  ◉ Listening         │
├────────────────────────────────────────────┤
│ Status / errors                            │
│ Recent mic lines (last 3, dimmed partial)  │
└────────────────────────────────────────────┘
```

### Subtitle overlay (700 × 120, always-on-top, dark opaque background)

```
 ─────────────────────────────────────────────
  [last 2 translated lines from Track 2]
  …live partial (italic, dimmed)
 ─────────────────────────────────────────────
```

Overlay is draggable (user can position it over the meeting window).
Falls back to opaque dark bg when transparency fails (X11/XFCE is common).

---

## Implementation order

### Step 1 — audio-os: add `capture_indefinite`
Fix the root cause of the panic. All subsequent steps depend on this.
Gate: `cargo test -p audio-os` passes; the dev binary `pw-capture-wav` still works.

### Step 2 — ui/src/config.rs
`AppConfig` struct: load from `.env` + `~/.config/realtime-translation/config.toml`.
Config covers all fields in `SessionConfig` that make sense to persist.
No UI yet; just the data model + load/save.

### Step 3 — ui/src/transcript.rs
`TranscriptLog`: open timestamped file on session start, append lines as events arrive.
Simple: one `BufWriter<File>` behind a `Mutex` (written from the async session task).

### Step 4 — ui/src/track.rs
Single-track runner: `spawn_track(TrackKind, config, stop_flag) -> TrackHandle`.
`TrackKind` = `Outgoing { tts_sink }` or `Incoming`.
Outgoing: mic capture → STT → translate → TTS → virtmic.
Incoming: sink-monitor capture → STT → translate → subtitle events only.
Both emit the same `TrackEvent` type.

### Step 5 — ui/src/session.rs
`SessionHandle`: spawns Track 1 (always), Track 2 (if enabled).
Owns the `stop_flag: Arc<AtomicBool>`.
Calling `session.stop()` sets the flag, which unblocks both capture threads.

### Step 6 — ui/src/state.rs
`UiState`: subtitle lines per track, partials, session status, last error.
Pure data — no egui imports, no tokio.

### Step 7 — ui/src/app.rs + ui/src/lib.rs
`TranslatorApp`: eframe App impl.
Control window wired to `UiState` + `SessionHandle`.
Subtitle overlay using `show_viewport_immediate`.
lib.rs: only re-exports.

### Step 8 — src/main.rs cleanup
Ensure tokio runtime size is sane; pass `AppConfig` into `TranslatorApp::new`.
