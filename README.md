# Realtime Translator

A Linux desktop application that translates your voice and incoming audio in real time using speech-to-text, neural machine translation, and text-to-speech — all streaming, low-latency, and running as a native binary.

## What it does

The app runs two parallel audio pipelines simultaneously:

**Track 1 — Outgoing mic (your voice)**
Your microphone is captured via PipeWire, streamed to Deepgram for live speech-to-text, translated by DeepL, and then spoken back through ElevenLabs TTS into a virtual microphone that meeting clients (Zoom, Google Meet, Teams, etc.) can pick as their input device. Your conversation partner hears your translated voice in near real time.

**Track 2 — Incoming audio (system playback)**
Whatever is playing through your speakers or headphones — the other person speaking — is captured via a PipeWire sink monitor, transcribed by Deepgram, and translated by DeepL. The translated text appears in a floating subtitle overlay window that sits always-on-top of your other windows. No TTS on this track; subtitles only.

Both tracks run independently. Either can fail (e.g., if you skip TTS keys) without taking down the other. Each session writes a full timestamped transcript log to disk automatically.

## Requirements

### System

- **Linux** with **PipeWire** and **WirePlumber** running as the audio server
  - PipeWire 0.3.77 or newer (the `v0_3_77` feature is used)
  - Check: `pipewire --version` should print `0.3.77` or higher
  - Check: `systemctl --user status pipewire wireplumber` should show active/running
- **PipeWire development headers** and **clang** for the build:
  - Arch Linux: `sudo pacman -S pipewire clang pkg-config`
  - Debian/Ubuntu: `sudo apt install libpipewire-0.3-dev clang pkg-config`
  - Fedora: `sudo dnf install pipewire-devel clang pkg-config`

### Rust

- **Rust 1.85 or newer** — required for Cargo edition 2024 support
- Install or update via [rustup](https://rustup.rs):
  ```sh
  curl --proto '=https' --tlsv=1.2 -sSf https://sh.rustup.rs | sh
  rustup update stable
  rustc --version   # must show 1.85.0 or higher
  ```

### API Keys

You need accounts and API keys for the services below. Free tiers are available for all three.

| Service | Used for | Required |
|---------|----------|---------|
| [Deepgram](https://console.deepgram.com/) | Speech-to-text (both tracks) | Yes |
| [DeepL](https://www.deepl.com/pro-api) | Translation | Yes (without it, audio passes through untranslated) |
| [ElevenLabs](https://elevenlabs.io) | Text-to-speech for your voice | Only for Track 1 TTS |

## Build

Clone the repository and build a release binary:

```sh
git clone <repo-url>
cd realtime-translation
cargo build --release
```

The binary is written to `target/release/translator`. First build takes ~60–90 seconds (compiling PipeWire bindings, audio DSP, egui). Subsequent builds are seconds.

You can copy the binary anywhere and run it standalone — no Rust toolchain needed at runtime:

```sh
cp target/release/translator ~/bin/translator
~/bin/translator
```

Or run directly from the project directory:

```sh
cargo run --release --bin translator
```

## Virtual microphone setup (Track 1 TTS output)

For your translated voice to appear as a selectable microphone in meeting clients, you need to install a PipeWire loopback configuration once. This creates a pair of virtual audio nodes:

- `translator_virtmic_sink` — the app writes translated speech PCM here
- `translator_virtmic_source` — meeting clients (Zoom, Meet, Teams) see this in their mic picker

**Install:**

```sh
mkdir -p ~/.config/pipewire/pipewire.conf.d
cp crates/audio-os/pipewire-conf/translator-virtmic.conf \
   ~/.config/pipewire/pipewire.conf.d/translator-virtmic.conf

# Restart the PipeWire stack to load the new config
systemctl --user restart pipewire wireplumber pipewire-pulse
```

**Verify** the virtual nodes appeared:

```sh
cargo run --bin pw-list-nodes
# Should list "Translator Virtual Mic (sink)" and "Translator Virtual Mic (source)"
```

If you skip this step, Track 1 TTS will still work but output to your default speakers instead of a virtual mic.

## API key configuration

Create a `.env` file in the project root (or wherever you run the binary from):

```sh
DEEPGRAM_API_KEY=your_deepgram_key_here
DEEPL_API_KEY=your_deepl_key_here
ELEVENLABS_API_KEY=your_elevenlabs_key_here
VOICE_ID=your_elevenlabs_voice_id_here
```

The app loads `.env` automatically on startup. You can also export these as shell environment variables, or enter the keys directly in the UI — keys typed in the UI take effect for that session only and are not persisted.

**Finding your ElevenLabs Voice ID:** Go to your ElevenLabs account → Voices → click a voice → the ID is shown in the URL and on the voice detail page.

Alternatively, place settings in `~/.config/realtime-translation/config.toml` (created manually). Environment variables always override the config file.

## First run and settings walkthrough

Launch the app:

```sh
./target/release/translator
# or
cargo run --release --bin translator
```

A 640×720 control window opens. Configure it top-to-bottom before clicking Start.

---

### Audio Sources section

Click the **Audio Sources** header to expand it.

**Microphone**
Pick your real physical microphone from the dropdown. Do not leave this on "Default" unless you are confident your system default input is the correct mic — using "Default" may capture the wrong device (e.g., a webcam mic). Click **Refresh devices** if your mic is not listed.

> The dropdown lists all PipeWire Source nodes. Bluetooth headsets, USB mics, and built-in mics all appear here by their device description and internal node name.

**Mic translates to**
The language your speech will be translated into for Track 1 (the TTS output). This is what the other person will hear. Example: if you speak English, set this to `German` so the meeting client receives German speech.

**Incoming subtitles (Track 2)**
Toggle to enable/disable Track 2. When enabled, a subtitle overlay window appears during the session showing translated incoming audio in real time.

**Audio source** (Track 2)
Pick the audio output device whose playback you want to capture and translate. For a video call, pick the headphones or speaker output that carries the other person's voice. "Default sink monitor" captures whatever is playing on your system default output.

> This uses PipeWire's monitor capability — it taps the playback stream without affecting what you hear.

**Incoming translates to**
The language to translate incoming audio into for the subtitle overlay. Example: if the other person speaks German, set this to `English` so you can read their subtitles in English.

**TTS output sink**
The PipeWire sink node name that Track 1 TTS audio is written to. Leave this as `translator_virtmic_sink` if you followed the virtual microphone setup above. Clear it (leave blank) to route TTS to your default speakers instead.

---

### API Keys section

Click **API Keys** to expand it.

- **Deepgram (required)** — paste your Deepgram API key. Without this the app will not start.
- **DeepL (translation)** — paste your DeepL API key. Without this, source transcripts pass through untranslated.
- **ElevenLabs (TTS)** — paste your ElevenLabs API key. Without this and a Voice ID, Track 1 produces no audio output (transcription and translation still work, but no TTS).
- **Voice ID** — paste your ElevenLabs voice clone or library voice ID.

Keys entered in the UI are used for the current session only. To persist them, put them in the `.env` file.

---

### Advanced section

Click **Advanced** to expand it. These control the transcript buffer behavior — how the app decides when to send a chunk of transcribed text to DeepL and then to TTS.

| Setting | Default | Meaning |
|---------|---------|---------|
| **Silence flush (ms)** | 1500 | Flush the current buffer if no new words have arrived for this many milliseconds |
| **Min chars (punct)** | 30 | Flush on sentence-ending punctuation (`.?!…`) only if the buffer already has at least this many characters |
| **Max chars** | 240 | Force-flush the buffer when it reaches this many characters regardless of punctuation or silence |
| **Context sentences** | 5 | Number of prior translated sentences sent to DeepL as context for better consistency |
| **Overlay lines shown** | 3 | How many subtitle lines the Track 2 overlay window shows at once |

Leave these at defaults unless you are experiencing choppy or laggy translation.

---

### Starting a session

Once all settings are filled in, click **▶ Start** (green button).

- The button changes to **■ Stop** with a spinner and "Listening…" indicator.
- Track 1 begins: mic audio is streaming to Deepgram. Partial transcripts appear in grey italics below the status area. Finalized + translated lines accumulate in the "Your speech (Track 1)" scroll area.
- If Track 2 is enabled, the subtitle overlay window opens — a dark semi-transparent window you can drag to any position on screen.
- The log file path is shown below the Start/Stop button once the session begins.

Click **■ Stop** to end the session. Both tracks drain and shut down cleanly.

---

## Session logs

Every session writes a timestamped log file automatically:

**Location:** `~/.local/share/realtime-translation/sessions/`

**Filename format:** `YYYY-MM-DD_HH-MM-SS.log` (UTC timestamp at session start)

**Log line format:**
```
[2026-04-26T15:30:01Z] [MIC]   PARTIAL  hello there
[2026-04-26T15:30:02Z] [MIC]   SOURCE   hello there, friend.
[2026-04-26T15:30:02Z] [MIC]   DE       Hallo, Freund.
[2026-04-26T15:30:05Z] [AUDIO] PARTIAL  gut morgen
[2026-04-26T15:30:06Z] [AUDIO] SOURCE   guten Morgen...
[2026-04-26T15:30:07Z] [AUDIO] EN       Good morning...
```

| Column | Meaning |
|--------|---------|
| `[MIC]` | Track 1 event (your microphone) |
| `[AUDIO]` | Track 2 event (incoming system audio) |
| `PARTIAL` | Live in-flight word (not yet final — may be corrected) |
| `SOURCE` | Finalized source transcript after buffer flush |
| `DE` / `EN` / etc. | Translated text in the target language |

The current session's log path is shown in the control window while a session is running. Runtime diagnostic messages (connection status, errors, warnings) go to stdout/stderr — visible in the terminal you launched the app from, or captured by your system logger. To see verbose logs: `RUST_LOG=debug ./translator`.

## Optional: config file

For persistent settings without editing `.env`, create `~/.config/realtime-translation/config.toml`:

```toml
source_lang     = "en"    # Force STT source language. Omit for auto-detect.
t1_target_lang  = "DE"    # Track 1 translation target (DeepL language code)
t2_target_lang  = "EN"    # Track 2 translation target
tts_sink_name   = "translator_virtmic_sink"
track2_enabled  = true
context_sentences = 5

[buffer]
min_chars_for_punct_flush = 30
max_chars_before_flush    = 240
silence_flush_ms          = 1500
flush_on_utterance_end    = true
```

Environment variables and `.env` always override this file.

## Developer tools

The build includes several diagnostic binaries useful during setup:

```sh
# Verify PipeWire connection works
cargo run --bin pw-smoke

# List all PipeWire audio nodes with their names and classes
cargo run --bin pw-list-nodes

# Record 5 seconds from the default mic to out.wav
cargo run --bin pw-capture-wav

# Play a test tone through the virtual mic sink
cargo run --bin pw-virtmic-tone

# Test the STT pipeline reading raw PCM from stdin
cargo run --bin pipeline-stt-stdin
```

## Supported languages

- **Speech-to-text:** All languages Deepgram supports. Leave `source_lang` unset for automatic language detection across all sessions.
- **Translation:** All DeepL supported language pairs. Common target codes: `EN`, `DE`, `FR`, `ES`, `IT`, `NL`, `PL`, `PT`, `JA`, `ZH`.
- **TTS:** ElevenLabs `eleven_flash_v2_5` model supports 32 languages including all major European languages and Japanese.

## Architecture overview

```
PipeWire mic ──► Deepgram STT (WS) ──► DeepL translate ──► ElevenLabs TTS (WS) ──► virtmic source
                                                                                    [Track 1]

PipeWire sink monitor ──► Deepgram STT (WS) ──► DeepL translate ──► subtitle overlay
                                                                      [Track 2]
```

Three Rust crates:

| Crate | Role |
|-------|------|
| `audio-os` | PipeWire capture, playback, virtual mic, device enumeration |
| `pipeline` | Deepgram / DeepL / ElevenLabs streaming clients, transcript buffer |
| `ui` | egui/eframe control window, subtitle overlay, session lifecycle, config |
