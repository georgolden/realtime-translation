# Real-Time Meeting Translator — Design Document

**Status:** Draft for review
**Date:** 2026-04-25
**Target system:** Linux + PipeWire (verified: PipeWire 1.6.4, WirePlumber, XFCE, Arch-based)

This document is a *tree of possibilities*, not a fixed plan. Each module section ends with a **Risks / Forks** subsection that describes what we do if the primary path fails.

---

## 1. Goals

A native Linux desktop application that, during a video meeting:

- **Track 1 (Outgoing):** captures the user's microphone, transcribes (Deepgram), translates (DeepL), synthesises with the user's cloned voice (ElevenLabs Flash v2.5), and **routes the synthesised audio into the meeting as the user's microphone**.
- **Track 2 (Incoming):** captures the audio output of the meeting (browser tab), transcribes the other participants, translates to the user's language, and displays rolling subtitles.

User constraints:
- Bluetooth headphones must be supported (wired mic + BT headphones is the expected setup).
- Audio source selection should feel like OBS / `pavucontrol` (pick from a list).
- The user already has both backend (Rust) and API integration experience. The unfamiliar parts are PipeWire-level OS integration and desktop UI.

Non-goals (for now):
- End-to-end voice-to-voice models (no current model meets the latency + cloning requirement).
- Mobile, Windows, macOS.
- Long-form translation memory / glossaries beyond what DeepL provides.

---

## 2. Modular Architecture

Three independent crates inside one Cargo workspace. Each is testable in isolation. Each defines a small **port trait** so the others depend on the trait, not the implementation. This is the only abstraction layer we add — everything else is concrete.

```
realtime-translation/                 (workspace root)
├── Cargo.toml                        (workspace + binary)
├── crates/
│   ├── audio-os/                     (Module 1 — PipeWire integration)
│   ├── pipeline/                     (Module 2 — STT / Translate / TTS)
│   └── ui/                           (Module 3 — egui front-end)
├── src/bin/
│   ├── translator.rs                 (the user-facing app — wires all 3 together)
│   ├── pw-list-nodes.rs              (dev tool: list PW nodes)
│   ├── pw-capture-wav.rs             (dev tool: record N seconds to wav)
│   ├── pw-virtmic-tone.rs            (dev tool: write a 440Hz tone to virtmic)
│   ├── pipeline-stt-stdin.rs         (dev tool: STT a wav file, print transcript)
│   └── pipeline-tts-file.rs          (dev tool: synthesise text → wav file)
└── vendor/
    ├── pipewire/                     (submodule — C reference)
    └── pipewire-rs/                  (submodule — Rust bindings + examples)
```

### 2.1 Why this split

`audio-os` and `ui` are **"works or doesn't work"** modules: PipeWire either captures and routes audio correctly or it doesn't; the UI either renders subtitles and routes events correctly or it doesn't. Once they pass their gate they stay passed and rarely change.

`pipeline` is the opposite. Auth is trivial (API keys), but everything *after* connecting is a moving target: how to handle partial results from a streaming STT, when to flush a transcript buffer, how big the speaker's "thinking pause" is, when an utterance is "really" over, how to keep meaning across slow speakers, jitter buffer sizes for TTS playback. This is where almost all the iteration during real meetings will happen. So the testing strategy is intentionally asymmetric:

| Module | Failure mode | How we test |
|---|---|---|
| `audio-os` | Bindings broken, threading wrong, virtmic not routed | Integration tests against the live PipeWire daemon + dev binaries for visual confirmation |
| `pipeline` | Wrong handling of partial results, bad utterance boundaries, transcript buffer logic | A few smoke unit tests for protocol framing/JSON; everything else is **manual e2e** in real meetings (the user drives this) |
| `ui` | Threading model, repaint loop, overlay window behaviour | Manual / visual |

Each module is a separate crate so `cargo test -p audio-os` runs only the relevant tests, and you cannot accidentally call into PipeWire from the UI thread.

We deliberately **do not** mock Deepgram/DeepL/ElevenLabs. Mocked transports tell you the message frame parses; they don't tell you whether the partial-result heuristic feels right. The real thing is cheap enough to call directly and the user is the oracle.

---

## 3. Module 1 — `audio-os` (PipeWire integration)

### 3.1 Responsibilities

1. Enumerate audio sources, sinks and their monitors.
2. Capture audio from a chosen source (real mic *or* a sink monitor for "browser audio").
3. Provide a virtual microphone that other apps see as a normal input device.
4. Write synthesised PCM into that virtual microphone.
5. Notify the rest of the app when devices appear / disappear (e.g. user plugs in BT headphones mid-meeting).

The module does **not** know about Deepgram, DeepL, ElevenLabs, languages, subtitles, or UI. It only deals with PCM frames and PipeWire objects.

### 3.2 Public API (sketch)

```rust
// crates/audio-os/src/lib.rs
pub struct AudioOs { /* opaque */ }

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,            // node.name
    pub description: String,     // node.description (human label)
    pub media_class: MediaClass, // Source / Sink / SinkMonitor / StreamOutput
    pub default: bool,
}

pub enum MediaClass { Source, Sink, SinkMonitor, StreamOutput, Other }

#[derive(Debug, Clone)]
pub struct AudioFormat {
    pub sample_rate: u32,        // typically 48_000
    pub channels: u16,           // 1 or 2
    pub layout: SampleLayout,    // F32LE on PipeWire
}

impl AudioOs {
    pub fn new() -> Result<Self, AudioOsError>;

    pub fn list_nodes(&self) -> Vec<NodeInfo>;

    /// Subscribe to node add/remove events. Returns a receiver.
    pub fn watch_nodes(&self) -> tokio::sync::mpsc::Receiver<NodeEvent>;

    /// Start capturing from `target`. Frames are pushed into a SPSC ring
    /// buffer. The returned handle owns the stream; drop = stop.
    pub fn capture(
        &self,
        target: CaptureTarget,
        on_frame: Box<dyn FnMut(&[f32], &AudioFormat) + Send>,
    ) -> Result<CaptureHandle, AudioOsError>;

    /// Open the virtual mic sink for writing. PCM frames written here are
    /// played out to anything that captures the paired virtual source.
    pub fn open_virtmic_writer(
        &self,
        format: AudioFormat,
    ) -> Result<VirtMicWriter, AudioOsError>;
}

pub enum CaptureTarget {
    Default,                     // default source
    DefaultSinkMonitor,          // monitor of the default sink (= "all browser audio")
    NodeId(u32),                 // specific node by id
    NodeName(String),            // specific node by node.name
}
```

The `on_frame` callback fires from PipeWire's real-time thread. Inside we **only** push to a `ringbuf` SPSC; the consumer side runs in a tokio task.

### 3.3 The virtual microphone

We do not create the virtual mic from Rust code. We ship a PipeWire config file and ask the user to install it once (or do it via a one-shot `--install-virtmic` subcommand that copies the file and restarts the user's PipeWire stack).

`~/.config/pipewire/pipewire.conf.d/translator-virtmic.conf`:

```conf
context.modules = [
  { name = libpipewire-module-loopback
    args = {
      node.description = "Translator Virtual Mic"
      capture.props = {
        node.name        = "translator_virtmic_sink"
        media.class      = "Audio/Sink"
        audio.position   = [ FL FR ]
      }
      playback.props = {
        node.name        = "translator_virtmic_source"
        media.class      = "Audio/Source"
        audio.position   = [ FL FR ]
        node.passive     = true
      }
    }
  }
]
```

After `systemctl --user restart pipewire wireplumber`, the OS exposes:
- `translator_virtmic_sink` — we write PCM into this.
- `translator_virtmic_source` — Firefox/Chrome/Zoom show this in their mic picker.

`open_virtmic_writer` connects an output stream targeted at `translator_virtmic_sink` by `node.name`.

### 3.4 PipeWire crate dependency — risk verification ✅ DONE

`pipewire = "0.9.2"` is the current crate (Sept 2025, ~200k recent downloads). It links against `libpipewire-0.3` via `pkg-config` and uses `bindgen` (requires `clang`/`libclang` at build time).

**Verified on 2026-04-25** on this machine:
- `clang` 22.1.3 installed.
- `libpipewire-0.3` 1.6.4, `libspa-0.2` 0.2 (dev headers present).
- A 5-line smoke binary using `MainLoopRc` / `ContextRc` / `connect_rc` builds and connects to the live PipeWire daemon. Output: `OK: connected to PipeWire from Rust`.

So Module 1 Risk A (fork to raw `pipewire-sys` or C shim) is closed for v1. We proceed with the safe `pipewire` crate.

The crate's own examples (`audio-capture.rs`, `pw-mon.rs`, `streams.rs`, `tone.rs`) are vendored at `vendor/pipewire-rs/pipewire/examples/` and are essentially what each of our dev binaries (`pw-list-nodes`, `pw-capture-wav`, `pw-virtmic-tone`) is a structured subset of.

### 3.5 Risks / Forks

**Fork A — `pipewire` crate fundamentally broken on 1.6.4.** ✅ Closed by verification on 2026-04-25.

**Fork B — Real-time callback semantics make the safe Rust API too restrictive (e.g. we hit `Send`/`Sync` problems).**
Mitigation: the PipeWire C `pw_stream` API is callback-based; we can drop down to `unsafe` for the callback registration only and keep the rest of the module safe.

**Fork C — Virtual mic latency is unacceptable (the loopback module adds buffering).**
Mitigation: use `media.class = "Audio/Source/Virtual"` on a single `null-audio-sink` instead of the loopback module. Slightly less robust but lower-latency. Documented in `pipewire-rs` examples.

**Fork D — Bluetooth profile switching breaks audio when the BT headset's mic gets enumerated alongside the headphones.**
Mitigation: in our enumeration UI, expose a profile filter. Force A2DP-sink-only on the BT device by setting `device.profile = a2dp-sink` via `wpctl`. This is a documentation/runbook fix, not a code fix — but we should detect the situation (headset mic appearing) and warn the user.

**Fork E — User's distro has PulseAudio (not PipeWire).**
Out of scope for v1. We assume PipeWire and check `pactl info` output at startup. If the server is not PipeWire-backed, refuse to start with a clear error.

### 3.6 Tests

- **Unit tests (`cargo test -p audio-os`):** format conversion (i16 ↔ f32, channel mixing), `CaptureTarget` resolution against a fake registry. No PipeWire required.
- **Integration tests (`cargo test -p audio-os --test live`):** marked `#[ignore]` by default; run with `cargo test -p audio-os --test live -- --ignored`. They:
  - Connect to the live PipeWire daemon.
  - Confirm `list_nodes()` returns at least one source.
  - Capture 1 second from the default source, assert non-zero RMS.
  - Open `translator_virtmic_sink`, write a sine, confirm it appears on the paired source by capturing the source for 0.5s and checking RMS.
- **Manual binaries** (in `src/bin/`) for human-eye verification — each prints to stdout and is killable with Ctrl-C.

---

## 4. Module 2 — `pipeline` (STT / Translate / TTS)

> **This is the hot module.** Auth is trivial (API keys in env). Everything else — partial-result handling, transcript accumulation, when to flush, when to translate, jitter buffer for TTS — is a moving target that gets tuned during real meetings, not unit tests. Treat the API in §4.3 as a v1 sketch that will be refactored once the user has spent a few sessions with it on. The shape here is correct; the constants and heuristics are not.

### 4.1 Responsibilities

1. Streaming STT via Deepgram WebSocket. Connection stays open for the duration of a session; PCM frames are pushed in continuously.
2. **Transcript accumulation.** Deepgram emits `is_final: false` partials, `is_final: true` finals, `speech_final: true` (endpointing fired), and `UtteranceEnd` (hard end-of-speech). None of these maps cleanly to "the speaker has finished a meaningful chunk we should translate." The pipeline owns a **transcript buffer** that:
   - Accumulates final segments across short pauses so the meaning isn't fragmented when the speaker thinks mid-sentence.
   - Decides when to flush the buffer to translation (based on punctuation, accumulated length, time-since-last-word, `UtteranceEnd`).
   - May re-translate on update if a partial substantially extends the previous flush (TBD; depends on how it feels in real use).
   These heuristics are **the thing we're going to tune**.
3. Translation via DeepL — currently HTTP per spec, but DeepL also has a streaming WS endpoint we may switch to if HTTP RTT becomes a bottleneck. Either way, the call site is the same ("translate this chunk") so swapping is a one-file change.
4. Streaming TTS via ElevenLabs WebSocket with voice clone. Connection stays open across utterances with idle-keepalive frames; force `flush:true` at end-of-utterance.
5. Jitter buffer between EL output and the virtual mic, sized to absorb network jitter without adding noticeable extra latency.
6. Emit `partial` and `final` transcript events for the UI; emit PCM frames into the virtmic writer (Track 1 only).

### 4.2 Pipeline shape

```
                                  ┌────────────────────┐
                                  │   TranscriptEvent  │ ──► UI (subtitles)
                                  │  (Partial / Final) │
                                  └────────────────────┘
                                            ▲
                                            │
[f32 PCM in] ─► [resample 48k→16k mono PCM16] ─► [Deepgram WS] ─┐
                                            │                    │
                                            │   partials/finals  │
                                            │                    ▼
                                            │           ┌──────────────────┐
                                            │           │ TranscriptBuffer │   ← heuristics live here
                                            │           │  (accumulate +   │
                                            │           │   flush logic)   │
                                            │           └──────────────────┘
                                            │                    │
                                            │     "translate" ◄──┘
                                            │                    │
                                            │                    ▼
                                            │            [DeepL HTTP or WS]
                                            │                    │
                                            │                    ▼
                                            │           [ElevenLabs WS, clone voice]
                                            │                    │
                                            │                    ▼  (PCM frames)
                                            │           [Jitter buffer]
                                            │                    │
                                            │                    ▼
                                            │     [resample EL rate → 48k stereo]
                                            │                    │
                                            │                    ▼
                                            │       [audio-os virtmic writer]
                                            ▼
                                  [debug tee to disk, opt-in]
```

Track 1 (outgoing) runs the full chain. Track 2 (incoming) reuses everything up to and including translation, and emits transcript events only — no TTS, no virtmic.

### 4.3 Public API (sketch)

```rust
// crates/pipeline/src/lib.rs

pub struct Pipeline { /* ... */ }

pub struct PipelineConfig {
    pub deepgram: DeepgramConfig,
    pub deepl:    DeeplConfig,
    pub eleven:   ElevenConfig,
    pub source_lang: Lang,
    pub target_lang: Lang,
    pub buffer:   TranscriptBufferConfig,   // tunable; see §4.4
    pub jitter_ms: u32,                     // TTS jitter buffer (Track 1)
}

pub struct TranscriptBufferConfig {
    /// Flush if buffer ends in `.`/`?`/`!` and at least this many chars accumulated.
    pub min_chars_for_punct_flush: usize,
    /// Flush after this many chars even without punctuation.
    pub max_chars_before_flush:    usize,
    /// Flush if no new word arrived for this long (covers slow/thinking speakers).
    pub silence_flush_ms:          u32,
    /// Always flush on UtteranceEnd.
    pub flush_on_utterance_end:    bool,
}

pub enum TrackKind {
    Outgoing { voice_id: String, write_pcm: PcmSink },  // STT → buffer → translate → TTS → write_pcm
    Incoming,                                            // STT → buffer → translate (no TTS)
}

pub enum PipelineEvent {
    /// Live partial from Deepgram, not yet flushed. UI may show this dimmed.
    Partial   { track: TrackId, text: String },
    /// A finalised segment that hasn't been translated yet (cheap, fast).
    Finalised { track: TrackId, source_text: String },
    /// A buffer flush — source + translation, the "real" subtitle update.
    Flushed   { track: TrackId, source_text: String, translated: String },
    Error     { track: TrackId, error: String },
}

impl Pipeline {
    pub fn spawn(cfg: PipelineConfig) -> (PipelineHandle, mpsc::Receiver<PipelineEvent>);

    /// Start a track. Returned handle owns the WS connections + buffer state.
    pub fn add_track(
        &self,
        kind: TrackKind,
        format: AudioFormat,
    ) -> TrackHandle;
}

pub struct TrackHandle {
    pub fn push_pcm(&self, frames: &[f32]);   // non-blocking, drops on backpressure
    pub fn flush_now(&self);                  // manual flush trigger (UI button / hotkey)
}
```

### 4.4 Transcript buffer (the iterative part)

This is the heuristic layer between Deepgram and DeepL. Deepgram tells us when *it* thinks something is final; that's not the same as when *we* should translate. Translating too eagerly fragments meaning and burns DeepL chars; waiting too long makes the conversation lag.

The v1 default rules — all tunable from config, all expected to change:

1. Append every `is_final: true` segment to the buffer. Discard `is_final: false` partials (they're for UI display only).
2. Flush the buffer to translation when **any** of these triggers:
   - The buffer ends in `.`, `?`, `!`, or `…` *and* has at least `min_chars_for_punct_flush` chars (default 30).
   - The buffer length exceeds `max_chars_before_flush` (default 240) — prevents runaway accumulation.
   - No new word for `silence_flush_ms` (default 1500) — covers thinking pauses without losing the in-flight meaning.
   - `UtteranceEnd` from Deepgram (hard end-of-speech).
   - The user pressed a "flush now" hotkey (manual override).
3. After a flush, the buffer is cleared. The next final starts a new segment.

The buffer also keeps the **last partial** separately, only for UI display, so the user sees live progress on the subtitle track.

We deliberately do not do "speculative re-translation on partial updates" in v1. It might be worth adding once we see how DeepL-on-finals feels.

### 4.5 Deepgram WS specifics

- Endpoint and params per `realtime_translator_spec.md`.
- One persistent WS connection per track. PCM frames stream in continuously.
- We send 16-bit PCM at 16 kHz, mono. Resample with `rubato` from the 48 kHz f32 stereo we get from PipeWire.
- Track 2 (incoming subtitles) emits `Partial` events to the UI immediately for live feedback.
- Track 1 (outgoing) does not surface partials — only the buffer's `Flushed` events trigger TTS.

### 4.6 DeepL specifics

- v1 uses HTTP POST `/v2/translate` with `model_type=latency_optimized` per spec.
- The pipeline calls DeepL once per buffer flush. Stateless, no connection reuse besides reqwest's keep-alive.
- **Fork on DeepL WS:** if HTTP RTT (round trip ~100–150ms) becomes the dominant component, swap to DeepL's streaming endpoint. The pipeline boundary (`translate(chunk) -> Future<String>`) doesn't change.

### 4.7 ElevenLabs WS specifics

- One persistent WS connection per outgoing track, kept alive across utterances with `{"text":" "}` heartbeats every ~15s (auto-close at 20s).
- Init payload per spec.
- Request `output_format=pcm_44100` (or `pcm_22050`). Avoids MP3 decoding, saves ~50 ms.
- Force `flush:true` at end-of-utterance (after each buffer flush).
- Resample EL output to 48 kHz before handing to `audio-os` (PipeWire's native rate on this machine).
- Jitter buffer of 100–200 ms before writing to PipeWire — tunable.

### 4.8 No SDK crates

We use:
- `tokio-tungstenite` (with `rustls-tls-webpki-roots`) for both WS endpoints.
- `reqwest` for DeepL HTTP.
- `rubato` for resampling.

The `elevenlabs-sdk` crate (v0.1.0, 80 downloads, single author, Feb 2026) is not used — the WS protocol is ~30 lines of JSON and we control versioning ourselves.

### 4.9 Risks / Forks

**Fork A — DeepL `latency_optimized` quality is unacceptable for a target language.**
Per-language config. For DE we may switch to `prefer_quality_optimized`. Spec lists only DE/NL/IT/ES, so a small matrix.

**Fork B — DeepL HTTP RTT dominates total latency.**
Switch to DeepL's streaming WS endpoint. One-file change at the `translate()` call site.

**Fork C — ElevenLabs Flash v2.5 voice clone quality on DE is not good enough.**
Fall back to `eleven_turbo_v2_5` (slightly higher latency, better quality). Configurable per session.

**Fork D — Network jitter causes audio gaps in TTS playback to the virtual mic.**
Tune the jitter buffer (100–200 ms). Trade latency for smoothness.

**Fork E — Transcript buffer heuristics don't match how the user actually speaks.**
This is *expected* — the v1 numbers in §4.4 are a starting point. All buffer parameters are exposed in config (`config.toml`) and reloaded on app restart so iteration is fast.

**Fork F — Echo/feedback when the user hears the translated audio in headphones via the meeting and the meeting also picks it up from the virtmic.**
By design the user mutes the original mic in the meeting client. Lowering its volume (instead of muting) so the user still hears their own English at low volume can feel less disorienting — like a simultaneous interpreter; it depends on the user. Documented decision, no code.

**Fork G — Side-tone monitor (hear the translated output yourself).**
The user wants to hear the translation as it goes out so they know what was actually broadcast. We can route a copy of the EL output to the default sink in addition to the virtmic. Cost: one more PipeWire output stream. Not in v1, but a clear next step.

**Fork H — Mix original mic + translated audio into the virtmic.**
The user mentioned wanting to experiment with this. The idea: the meeting hears both the original English and the translated DE/NL/IT/ES, like a live interpreter on top of the speaker. Implementation is a mixer node in `audio-os` that writes `α·mic + β·tts` to the virtmic. **Not in v1** — voice-clone-only is simpler and a clear baseline. Reconsider after the user has live-tested v1.

### 4.10 Tests

- **A handful of unit tests** for the parts that are pure logic and unlikely to change:
  - Resampler edge cases (frame boundaries, channel-collapse 2→1).
  - JSON encode/decode of the WS frames against captured fixtures.
  - Transcript buffer state machine — given a sequence of mock Deepgram events, the right `Flushed`/`Partial` events come out. This is worth one focused test, not a suite.
- **No mocked HTTP/WS clients.** Mocking the transports would only verify what we already know (serialisation works); it can't tell us whether the buffer feels right.
- **Manual e2e is the primary test loop.** The user runs the app in a real meeting, observes behaviour, files a list of tweaks, and we iterate. The dev binaries (`pipeline-stt-stdin`, `pipeline-tts-file`) exist for inspecting individual stages without firing up the whole meeting setup.

---

## 5. Module 3 — `ui` (egui front-end)

### 5.1 Responsibilities

1. Main control window: pick mic, pick output capture (browser sink monitor), pick target language, start/stop.
2. Subtitle overlay: small always-on-top borderless window with the rolling translated text from Track 2.
3. Live status: track 1 / track 2 latency, last error, current voice ID.

### 5.2 Why egui

- Single-window apps with simple controls and a separate overlay = egui's sweet spot.
- No DSL, plain Rust.
- Plays nicely with tokio: the audio + network code runs in tokio; egui receives `PipelineEvent`s over a `tokio::sync::mpsc::Receiver` and calls `ctx.request_repaint()`.
- Small binary, no Electron, no GTK theming pitfalls on XFCE.

`iced` and `slint` are alternatives if we later want a more designed look; not needed for v1.

### 5.3 Threading model

```
tokio runtime (multi-thread):
  ├─ pipeline tasks (DG WS, DeepL, EL WS, resamplers)
  └─ audio-os event watcher (relays NodeEvent to UI)

native thread:
  └─ egui main loop (eframe)
        ├─ owns: PipelineEvent receiver, NodeEvent receiver
        └─ on each frame: drain receivers, mutate state, request_repaint

PipeWire RT thread (managed by pipewire-rs):
  └─ on_frame callback → ringbuf → tokio task picks it up
```

Critical rules:
- The PW RT thread never touches tokio, never allocates, never blocks.
- The egui thread never blocks on network I/O.

### 5.4 Risks / Forks

**Fork A — Subtitle overlay doesn't behave well on XFCE (transparency, always-on-top).**
Mitigation: egui's `viewport.always_on_top` + `viewport.transparent` work on most X11/Wayland setups. If XFCE's compositor causes issues, fall back to an opaque dark window with a window manager rule.

**Fork B — Egui's threading model conflicts with how we want to push events.**
Mitigation: egui supports `Context::request_repaint_after` and external repaint triggers. Worst case we poll every 50 ms.

### 5.5 Tests

- State-machine unit tests on the `UiState` struct (no rendering).
- Manual visual verification — UI is too thin to merit a test framework.

---

## 6. Build order (the actual plan we'll follow)

This is ordered by **risk**, hardest first. Each step ends with a concrete demoable artefact.

### Stage 0 — Workspace skeleton + smoke test ✅ smoke verified, scaffolding pending
- Smoke binary already proven to build and connect (verified externally on 2026-04-25).
- Remaining work: convert the single-crate project into a Cargo workspace; add empty `crates/audio-os`, `crates/pipeline`, `crates/ui`; commit `src/bin/pw-smoke.rs` as the canonical entry to the smoke test.
- **Gate:** `cargo run --bin pw-smoke` prints `OK: connected to PipeWire from Rust`.

### Stage 1 — Module 1: enumeration
- `audio-os::list_nodes()` and `watch_nodes()`.
- `src/bin/pw-list-nodes.rs` — prints all nodes, like `pactl list short`.
- Tests: integration test that asserts at least the default source appears.
- **Gate:** binary lists user's mic, sink monitors, browser streams.

### Stage 2 — Module 1: capture
- `audio-os::capture()` for any target.
- `src/bin/pw-capture-wav.rs` — captures N seconds from a chosen target into a `.wav`.
- Tests: integration test that captures default source for 1 s and asserts non-zero RMS.
- **Gate:** captured wav of "browser sink monitor" plays back the same audio.

### Stage 3 — Module 1: virtual mic
- Ship the `.conf` file + a `--install-virtmic` subcommand in the main binary.
- `audio-os::open_virtmic_writer()`.
- `src/bin/pw-virtmic-tone.rs` — writes a 440 Hz sine tone to `translator_virtmic_sink`.
- **Gate:** open Firefox, in `chrome://settings` pick "Translator Virtual Mic" — hear/see the tone.

> After Stage 3, **Module 1 is done and proved end-to-end.** Modules 2 and 3 are now mostly Rust/API work in your wheelhouse.

### Stage 4 — Module 2: STT only + transcript buffer
- Deepgram WS client, persistent connection.
- Implement the transcript buffer (§4.4).
- `pipeline-stt-stdin` binary: pipe PCM from stdin (or a wav file via the dev binary), print `Partial`/`Finalised`/`Flushed` events with timestamps.
- **Gate (manual):** speak naturally into the mic via Stage 2's capture binary piped into this; the Flushed events arrive at sentence boundaries, partials show live progress, slow speakers don't get fragmented.

### Stage 5 — Module 2: STT + DeepL
- Wire DeepL HTTP behind every buffer flush.
- **Gate (manual):** speak in English, watch the terminal print English source + DE translation aligned.

### Stage 6 — Module 2: full Track 1 with TTS
- ElevenLabs WS, jitter buffer, write synthesised PCM into the virtmic.
- **Gate (manual):** join a meeting in Firefox, pick "Translator Virtual Mic" as the input, speak in English, listener on the other end hears German in the user's cloned voice.

### Stage 7 — Module 2: Track 2
- Capture default sink monitor → same buffer/translate path → emit transcript events.
- Print to stdout for now.
- **Gate (manual):** play a German YouTube video, see English subtitles in the terminal.

> After Stage 7 there'll be a tuning pass: the user runs the app in actual meetings and we adjust the buffer constants (§4.4), jitter buffer size, EL model choice, etc. This is expected to be the bulk of the work even though it doesn't add features.

### Stage 8 — Module 3: UI
- egui control window: source/sink picker, language picker, start/stop, "flush now" button/hotkey.
- Subtitle overlay: borderless, always-on-top window for Track 2 translations.
- **Gate:** full app usable through the GUI; can run a meeting end-to-end without touching the terminal.

---

## 7. System dependencies

```
pacman -S pipewire pipewire-pulse wireplumber pipewire-audio \
          base-devel pkgconf clang
```

`clang` is required at build time for `bindgen` (used by `pipewire-sys`).

Verified present on this machine: `pipewire` 1.6.4, `pactl` 17.0, `wireplumber`, `libpipewire-0.3` (1.6.4), `libspa-0.2`. Missing: `clang` (must install before Stage 0 smoke test passes).

---

## 8. Crate dependencies (workspace)

```toml
# Workspace root
[workspace]
resolver = "3"
members  = ["crates/audio-os", "crates/pipeline", "crates/ui"]

[workspace.dependencies]
# OS/audio
pipewire = { version = "0.9.2", features = ["v0_3_77"] }
libspa   = "0.9"

# Async + net
tokio              = { version = "1", features = ["full"] }
tokio-tungstenite  = { version = "0.27", features = ["rustls-tls-webpki-roots"] }
reqwest            = { version = "0.12", features = ["json", "rustls-tls"] }
futures            = "0.3"
futures-util       = "0.3"

# Audio plumbing
ringbuf  = "0.4"
rubato   = "0.16"
hound    = "3.5"   # WAV I/O for dev binaries and tests

# Misc
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
toml        = "0.8"          # config.toml parsing
dotenvy     = "0.15"         # .env loader for API keys
anyhow      = "1"
thiserror   = "2"
log         = "0.4"
env_logger  = "0.11"
bytes       = "1"

# UI
egui   = "0.32"
eframe = "0.32"
```

`elevenlabs-sdk` is intentionally **not** used (see §4.8). `cpal` is intentionally not used (we go direct to PipeWire).

---

## 9. Decisions (resolved during review)

1. **UI layout.** Main control window **+** always-on-top subtitle overlay. The overlay is the answer to "what does 'overlay' mean": a borderless, always-on-top window that floats over the meeting client showing rolling Track 2 translations.
2. **Voice clone.** ElevenLabs Instant Voice Clone (IVC), created once via the EL dashboard, `VOICE_ID` stored in env or config.
3. **Configuration.**
   - **Secrets** (`DEEPGRAM_API_KEY`, `DEEPL_API_KEY`, `ELEVENLABS_API_KEY`, `VOICE_ID`) live in `.env` at the project root, loaded with the `dotenvy` crate. Already the convention here.
   - **Everything else** (default source/sink, target language, transcript-buffer constants, jitter buffer size, EL model, TTS-output sample rate) lives in `~/.config/realtime-translation/config.toml`. Re-read on start.
4. **Distribution.** No CI. Single-developer, single-machine project. We use `cargo build` and `cargo run --bin <name>` for local iteration. The end-user app is the `translator` binary built locally.
5. **Logging.** Start with `env_logger` to stderr — already in `Cargo.toml`. If/when stderr becomes inconvenient we add a file appender at `~/.local/state/realtime-translation/log.txt`. Not blocking v1.

---

## 10. Vendored references

Both kept up to date manually with `git submodule update --remote --merge`:

- `vendor/pipewire/` — official PipeWire C source, used as documentation. Examples in `src/examples/` and `pipewire/examples/audio-capture.c` map 1:1 to our `pw-capture-wav` binary.
- `vendor/pipewire-rs/` — official Rust bindings. Useful files:
  - `pipewire/examples/audio-capture.rs` — direct reference for our `audio-os::capture()` implementation.
  - `pipewire/examples/pw-mon.rs` — reference for our `list_nodes()` / `watch_nodes()`.
  - `pipewire/examples/tone.rs` — reference for our `open_virtmic_writer()` (output stream).
  - `pipewire/examples/streams.rs` — alternative stream patterns.
  - `pipewire/Cargo.toml` `[features]` — version flags map (we use `v0_3_77`, system has 1.6.4 = newer).
