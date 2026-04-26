# Manual Testing Runbook

How to drive every dev binary and integration test by hand, what to look for in
the output, and what "passes the gate" means at each stage.

Run everything from the project root:

```
cd /home/jebuscross/projects/realtime-translation
```

Tools you'll want installed for verification:
- `pactl`, `pw-cli` — already on your system (PipeWire / pulse-compat).
- `ffprobe`, `ffmpeg` — for reading wav metadata and measuring signal levels.
- `paplay` (or any audio player) — for ear-checking captured wavs.

---

## Build first, run second

The first build pulls in PipeWire, libspa, hound, etc. (~30 s). Subsequent
builds are seconds.

```
cargo check --workspace --tests          # fast: catches compile errors
cargo build --workspace                   # full debug build
```

If `cargo check` fails the rest of this doc won't help — fix that first.

---

## Stage 0 — `pw-smoke`

**Purpose:** prove the `pipewire` Rust crate links against your installed
PipeWire and can connect to the running daemon. Five lines of code; if this
doesn't work we'd have to fork to raw `pipewire-sys`.

**Run:**

```
cargo run --bin pw-smoke
```

**Expected output:**

```
OK: connected to PipeWire from Rust
```

**If it fails:**

| Symptom | Likely cause | Fix |
|---|---|---|
| `Unable to find libclang` during build | `clang` not installed | `sudo pacman -S clang` |
| `pkg-config` errors at build | dev headers missing | `sudo pacman -S pipewire` (provides `libpipewire-0.3`) |
| Builds but exits with `pipewire error: ...` at runtime | daemon not running | `systemctl --user status pipewire` and start it |

---

## Stage 1 — `pw-list-nodes`

**Purpose:** prove `audio-os::list_nodes()` enumerates the registry and
classifies audio nodes correctly.

**Run:**

```
cargo run --bin pw-list-nodes
```

**Expected output (rows depend on what's connected):**

```
   id  class           name                                              description
------------------------------------------------------------------------------------------------------------------------
   45  source          bluez_input.41:42:FF:8A:6D:53                     YYK-Q39
   69  sink            bluez_output.41_42_FF_8A_6D_53.1                  YYK-Q39
   78  stream-out      Chromium                                          Chromium
   34  other           bluez_capture_internal.41:42:FF:8A:6D:53          Bluetooth internal capture stream for YYK-Q39

4 audio nodes
```

**What to check:**

1. Every audio device you have connected appears — wired mic as `source`,
   speakers/headphones as `sink`, BT headset as both, browser tabs playing
   audio as `stream-out`.
2. Cross-check against `pactl list short sources` and `pactl list short sinks`
   — anything `pactl` shows that is *not* `*.monitor` should appear in our
   list. Monitors are not separate nodes in PipeWire 1.6.x; they're ports on
   the sink. So `auto_null.monitor` in `pactl` ≈ `auto_null` sink in our list.
3. If a node shows up as `other` it just means PipeWire's `media.class` for it
   is something we don't categorise (e.g. `Stream/Input/Audio/Internal` for
   BT internal capture). Not a bug.

**Live integration tests (same crate):**

```
# Default `cargo test` skips these — they need a live PipeWire daemon.
cargo test -p audio-os                                   # unit tests only
cargo test -p audio-os --test list_nodes_live -- --ignored  # live tests
```

Both should pass: at-least-one-audio-node + at-least-one-sink-present.

---

## Stage 2 — `pw-capture-wav`

**Purpose:** prove `audio-os::capture_for_duration()` records real PCM frames
from a PipeWire node into a wav file.

### Usage

```
pw-capture-wav OUT.wav                              # default source, 5s
pw-capture-wav OUT.wav --secs 10                    # custom duration
pw-capture-wav OUT.wav --node ID                    # capture a specific node
pw-capture-wav OUT.wav --sink-monitor ID            # capture what a sink is playing
```

`ID` is the integer from `pw-list-nodes`. Find the node first, then capture.

### Test 1 — record yourself speaking (default mic path)

This is the canonical end-to-end check for Stage 2.

```
# Find your mic id (look for class=source):
cargo run --bin pw-list-nodes

# Capture 5 seconds. Speak normally during the window.
cargo run --bin pw-capture-wav -- /tmp/cap-mic.wav --secs 5
```

**Expected log lines:**

```
[INFO  pw_capture_wav] capturing for 5.0s to /tmp/cap-mic.wav (target: Default)
[INFO  audio_os::capture] negotiated format: rate=48000 channels=1
[INFO  pw_capture_wav] wrote N samples → /tmp/cap-mic.wav
```

`N` should be roughly `seconds × rate × channels`, minus a small startup
fragment (e.g. 5 s × 48000 × 1 ≈ 240 000; expect ~230k). The exact rate /
channel count depends on your device — BT in HFP gives 48 kHz mono, wired
mics often give 48 kHz stereo, etc.

**Verify the file:**

```
# 1. Sanity-check the header.
ffprobe -v error -show_streams -show_format /tmp/cap-mic.wav | head -20
#   sample_fmt=flt
#   sample_rate=48000
#   channels=1 (or 2)
#   duration=~5.0

# 2. Confirm there's actual signal (not silence).
ffmpeg -hide_banner -nostats -i /tmp/cap-mic.wav -af volumedetect -f null /dev/null 2>&1 \
    | grep -E "mean_volume|max_volume"
#   max_volume should be well above -90 dB if you spoke (typical: -10 to -40 dB).
#   A silent room reads -65 to -90 dB.

# 3. Listen to it.
paplay /tmp/cap-mic.wav
#   You should hear yourself. Quality matches the device — BT HFP is narrow-band
#   and sounds tinny; that's expected, not a capture bug.
```

**Pass:** ffprobe shows the right format, volumedetect's `max_volume` is loud
when you spoke and quiet when you didn't, and `paplay` plays back what you
said.

### Test 2 — capture a specific node by id

Same as above but explicit. Useful when multiple sources are connected and
you want to be sure you're recording the right one.

```
# Suppose pw-list-nodes shows id 45 = bluez_input...
cargo run --bin pw-capture-wav -- /tmp/cap-bt.wav --node 45 --secs 3
```

Expected: log line `target: Node(45)`, file recorded with the same checks as
Test 1.

### Test 3 — capture what a sink is playing (browser audio path)

This is the path Track 2 of the app uses for "subtitle other people in the
meeting".

```
# Start playing audio in a browser (any YouTube video, music, etc.). Find the
# sink's id (class=sink, the one that's currently active):
cargo run --bin pw-list-nodes

# Capture 5 seconds of what's playing on that sink:
cargo run --bin pw-capture-wav -- /tmp/cap-browser.wav --sink-monitor SINK_ID --secs 5
```

**Expected log lines:**

```
[INFO  pw_capture_wav] capturing for 5.0s to /tmp/cap-browser.wav (target: SinkMonitor(SINK_ID))
[INFO  audio_os::capture] negotiated format: rate=48000 channels=2
[INFO  pw_capture_wav] wrote N samples → /tmp/cap-browser.wav
```

Sinks are typically stereo, so expect channels=2 here.

**Verify:** `paplay /tmp/cap-browser.wav` should reproduce what was playing
in the browser. If the file is silent (`max_volume` at -90 dB) but the
browser was audibly playing, you targeted the wrong sink — re-run
`pw-list-nodes` and check that the sink id matches the device that's
actually playing. PipeWire reassigns the default sink when you connect
headphones, so the id can change.

### Common issues at Stage 2

| Symptom | Cause | Fix |
|---|---|---|
| `wrote 0 samples` | Source has no signal at all (mic disconnected, sink not playing) | Check `pactl list short sources` shows the source as `RUNNING` not `SUSPENDED` |
| `dequeue_buffer returned None (out of buffers)` warning, occasionally | Brief glitch on the loop, normally harmless | Ignore unless it floods the log |
| File is silent but the source was active | Wrong target id, or BT headset was in the wrong profile | Re-list nodes; for BT, `wpctl status` shows the active profile |
| `negotiated format: rate=...` never logged | Stream never connected / format never negotiated | Source died before producing audio; try a different `--node` |

---

## Stage 3 — `pw-virtmic-tone` + virtual mic install

**Purpose:** Stage 3 has two parts. First we install a PipeWire loopback
module that creates `translator_virtmic_sink` (an audio sink we write into)
and `translator_virtmic_source` (a fake microphone other apps can pick).
Then we use `pw-virtmic-tone` to write a 440 Hz sine into the sink and
confirm Firefox / Chromium / any meeting client sees it as a usable mic.

### Part 0 — sanity test the writer alone (no install needed)

Before touching the OS, confirm that `audio-os::play_for_duration` actually
plays audio. Run with no flags — it plays to the **default sink** (your
headphones).

```
cargo run --bin pw-virtmic-tone -- --secs 2 --volume 0.1
```

**Expected:** you hear a quiet 440 Hz beep for 2 seconds. Logs show:

```
[INFO  pw_virtmic_tone] playing 440.0 Hz tone for 2.0s (volume 0.10) to Default
[INFO  audio_os::playback] playback negotiated format: rate=48000 channels=2
[INFO  pw_virtmic_tone] done
```

If you don't hear anything but logs look fine, your default sink is wrong —
check `wpctl status` to see which sink is the default.

### Part 1 — install the virtual microphone

This is a one-time OS-level setup. The config file is committed in the repo
at [crates/audio-os/pipewire-conf/translator-virtmic.conf](../crates/audio-os/pipewire-conf/translator-virtmic.conf).

```
# 1. Make the user-level PipeWire config dir if it doesn't exist.
mkdir -p ~/.config/pipewire/pipewire.conf.d

# 2. Copy the config in.
cp crates/audio-os/pipewire-conf/translator-virtmic.conf \
   ~/.config/pipewire/pipewire.conf.d/

# 3. Restart the user PipeWire stack. NOTE: this momentarily kills all your
#    audio. BT headsets may briefly disconnect and re-pair.
systemctl --user restart pipewire wireplumber pipewire-pulse

# 4. Verify the new nodes exist.
cargo run --bin pw-list-nodes
```

**Expected after step 4** — two new rows:

```
   id  class           name                              description
   ?   source          translator_virtmic_source         Translator Virtual Mic
   ?   sink            translator_virtmic_sink           Translator Virtual Mic
```

(IDs are assigned by PipeWire and change across restarts; that's why the
rest of the app refers to them by `node.name`.)

Cross-check via pactl:

```
pactl list short sources | grep translator
pactl list short sinks | grep translator
```

Both should print one line each.

**To uninstall** (e.g. if you want to try a different config):

```
rm ~/.config/pipewire/pipewire.conf.d/translator-virtmic.conf
systemctl --user restart pipewire wireplumber pipewire-pulse
```

### Part 2 — write a tone into the virtmic and verify in a browser

**Run, in one terminal:**

```
cargo run --bin pw-virtmic-tone -- \
    --node-name translator_virtmic_sink \
    --secs 30 \
    --volume 0.2
```

The tone runs for 30 seconds. During that window:

1. Open Firefox or Chromium.
2. Go to a site that needs the mic — `https://webcammictest.com/check-microphone.html`
   is a good one (no signup, shows live waveform).
3. When prompted for mic permission, the browser will show its mic picker.
   You should see **"Translator Virtual Mic"** in the list.
4. Pick it. The site's level meter should immediately show a steady tone
   (since a sine wave is constant amplitude, the meter pegs at one level).

**Pass:** the tone is visible/audible in the browser test, your voice is
**not** going through (because we picked the virtual mic, not the real one).

### Part 3 — verify the loopback chain end-to-end (no browser)

This is the most reliable test of the full chain — completely independent
of any browser quirks (Chromium's WebRTC noise suppression, for example,
will gate a pure 440 Hz sine to silence even though the mic itself works
perfectly). Use this whenever something in the browser looks off.

**Two terminals.** In **terminal A**, start the tone (foreground):

```
cargo run --bin pw-virtmic-tone -- --node-name translator_virtmic_sink --secs 30 --volume 0.5
```

In **terminal B**, while it's running:

```
# 1. Confirm our stream is connected to the sink.
pw-link -i 2>&1 | grep -i translator
# Expect:  translator_virtmic_sink:playback_FL
#          translator_virtmic_sink:playback_FR

# 2. Find the source's id.
cargo run --bin pw-list-nodes
#   Look for translator_virtmic_source (class=source). Note its id.

# 3. Capture from the source side and measure signal.
cargo run --bin pw-capture-wav -- /tmp/cap-virtsrc.wav --node SOURCE_ID --secs 3
ffmpeg -hide_banner -nostats -i /tmp/cap-virtsrc.wav -af volumedetect -f null /dev/null 2>&1 \
    | grep -E "mean_volume|max_volume"
```

**Pass:** with `--volume 0.5`, expect roughly:

```
mean_volume: -9.0 dB
max_volume:  -6.0 dB
```

(A 0.5 amplitude sine has −6.02 dB peak by definition. Anything close
means the loopback is forwarding cleanly.)

**Listening to the file:**

```
paplay /tmp/cap-virtsrc.wav
```

⚠️ `paplay` plays to the **default sink**, which may not be the device
you're listening on. PipeWire often reassigns the default sink when you
plug in a USB device — and some USB mic dongles also expose an output
endpoint that PipeWire then promotes to default. If `paplay` produces
silence but the volumedetect numbers above were correct, the file is
fine; you just played it to the wrong device. Two ways to check:

```
# Which sink is currently default?
pactl info | grep "Default Sink"

# Force playback to a specific sink (e.g. the BT headphones):
paplay --device=bluez_output.41_42_FF_8A_6D_53.1 /tmp/cap-virtsrc.wav

# Or change the default sink for this session:
wpctl set-default $(pw-cli ls Node | grep -B1 'bluez_output' | head -1 | awk '{print $2}' | tr -d ',')
```

A common Linux gotcha: a USB headset/mic dongle registers as both
`alsa_input.usb-...` (the mic) and `alsa_output.usb-...` (a built-in
speaker the dongle technically has, even if there's no speaker plugged
into it). When PipeWire sees the new output it switches the default
sink to it, and your normal speakers go silent.

### Part 4 (optional) — verify in a browser

This is a "nice to have" check but Chromium specifically interferes:
its default `getUserMedia` constraints enable WebRTC noise suppression,
auto gain, and echo cancellation, which **delete a pure tone** because
they treat steady audio as background noise. If the browser shows no
signal but Part 3 passed, the browser is doing this — it's not a bug
in our code.

To bypass the WebRTC processing, use the WebRTC samples page that
exposes the constraints as checkboxes:

> **https://webrtc.github.io/samples/src/content/getusermedia/audio/**

1. Pick "Translator Virtual Mic (source — selectable in meeting clients)"
   from the dropdown.
2. Untick **all three** audio-processing checkboxes
   (Echo cancellation, Noise suppression, Auto gain).
3. The waveform should now show your sine cleanly.

For the actual app (Stage 6+) we'll send speech, not tones, so WebRTC
processing is no longer destructive — speech has the spectral richness
that fools the noise suppressor. So it's fine to consider Stage 3 done
even if Part 4 doesn't work perfectly.

### Common issues at Stage 3

| Symptom | Cause | Fix |
|---|---|---|
| After `systemctl restart`, no audio anywhere | Service didn't come back up | `systemctl --user status pipewire` — restart again or check journal |
| `pw-list-nodes` doesn't show the virtmic | Config syntax error or wrong path | `journalctl --user -u pipewire -n 50` for parser errors; verify path is `~/.config/pipewire/pipewire.conf.d/translator-virtmic.conf` |
| `paplay /tmp/cap-virtsrc.wav` is silent but volumedetect showed signal | Default sink got reassigned to a device that isn't audible (e.g. a USB mic dongle that also exposes a fake speaker endpoint) | `pactl info \| grep "Default Sink"` to see; play with `paplay --device=NODE_NAME` or run `wpctl set-default ID` to switch back |
| Browser doesn't list "Translator Virtual Mic" | Browser cached the old mic list | Reload the page; some sites need the tab fully restarted |
| Tone runs, sink shows signal in pavucontrol, but Chromium-based browser shows no signal | WebRTC noise suppression / AGC strips out steady tones | Use the WebRTC samples page (linked in Part 4) and untick all 3 audio-processing checkboxes; or skip — speech in Stage 6+ won't be affected |
| `cargo run --bin pw-virtmic-tone -- --node-name translator_virtmic_sink` fails to connect | Virtmic not installed yet, or service didn't load it | Re-do Part 1 |
| BT headset stuck in HFP after restart and sounds awful | WirePlumber profile autoswap | `wpctl status` shows the BT device's active profile; switch to A2DP via `wpctl set-profile DEVICE_ID INDEX` |

---

## Stage 4 — `pipeline-stt-stdin`

**Purpose:** prove that the Deepgram WS client streams PCM in correctly,
the result-message parser handles partials/finals/UtteranceEnd, and the
transcript buffer (DESIGN §4.4) flushes at sentence boundaries instead of
fragmenting on every Deepgram final.

This stage has no integration with the OS beyond capture (already proved
in Stage 2). What's new and worth verifying by ear is the **buffer
heuristic**: a long sentence with mid-utterance pauses should land as
**one** `FLUSHED` line, not a torrent of them.

### Setup

You need a Deepgram API key. Put it in `.env` at the project root:

```
DEEPGRAM_API_KEY=...
```

`.env` is git-ignored. The binary loads it via `dotenvy::dotenv()`.

### Language detection

By default the binary uses `language=multi`, which is nova-3's
multilingual mode — it auto-detects the language within the stream
and can handle mid-conversation language switches. This is the correct
streaming equivalent of language detection. (Deepgram's
`detect_language=true` param only works for pre-recorded audio, not
streaming, and causes a 400 Bad Request on a WebSocket connection.)

Pass `--language de` (or any BCP-47 code) to fix the language and
skip detection. There is no `--detect-language` flag; `multi` is the
default.

```
# Auto-detect / multilingual (default):
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav

# Fixed language, no detection:
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav --language de
```

### Test 1 — wav file replay (stable, no PipeWire)

Use a wav recorded via Stage 2's `pw-capture-wav`. The binary paces it
at real-time so Deepgram's endpointing fires the same way it would on a
live mic.

```
# Record a 10-second clip of yourself speaking 2–3 sentences with
# clear pauses between them. Speak naturally — don't rush.
cargo run --bin pw-capture-wav -- /tmp/utt.wav --secs 10

# Replay through Deepgram (auto-detect language):
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav

# Or with a fixed language:
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav --language de
```

**Expected output shape (timestamps in ms from start of run):**

```
[INFO ...] Deepgram model=nova-3 language=en; buffer punct>=30 max>=240 silence=1500ms
[INFO ...] Deepgram: connecting to wss://api.deepgram.com/v1/listen
[INFO ...] Deepgram: connected
[INFO ...] wav: 48000 Hz, 1 ch, 32 bits, ...
[   850 ms] PARTIAL    hello
[  1100 ms] PARTIAL    hello there
[  1350 ms] FINALISED  hello there friend.
[  1360 ms] FLUSHED    (punctuation)  hello there friend.
[  3200 ms] PARTIAL    so what
[  3450 ms] PARTIAL    so what i wanted to say
[  4100 ms] FINALISED  so what i wanted to say is the buffer should hold this together.
[  4110 ms] FLUSHED    (punctuation)  so what i wanted to say is the buffer should hold this together.
[INFO ...] wav: streaming complete
[INFO ...] Deepgram: ws closed
```

**What to check:**

1. `PARTIAL` lines arrive *during* speech — Deepgram reports interim
   transcripts well before you finish a sentence.
2. `FINALISED` is logged when Deepgram marks `is_final=true`, **before**
   the buffer's flush trigger fires.
3. `FLUSHED` lines correspond to natural sentence boundaries, not every
   tiny chunk Deepgram finalised. The reason in parens tells you which
   trigger fired:
   - `punctuation` — buffer ended in `.`/`?`/`!`/`…` and ≥30 chars.
   - `max-chars`   — runaway sentence hit 240 chars.
   - `silence`     — ≥1.5 s of no new word (mid-thought pause).
   - `utterance-end` — Deepgram's hard end-of-speech.
   - `manual`      — only fires from a hotkey (no UI yet).
4. The trailing `manual` flush at EOF ensures words near the end of the
   file aren't silently dropped.

### Test 2 — live mic

Same path but with PipeWire capture instead of a wav file. Useful as a
realistic feel-test before tuning the buffer constants.

```
# Auto-detect language (default):
cargo run --bin pipeline-stt-stdin -- --mic --secs 30

# Fixed language:
cargo run --bin pipeline-stt-stdin -- --mic --secs 30 --language de
```

Speak naturally for the duration. Try both:
- A single fluent sentence (expect one `FLUSHED (punctuation)`).
- A halting sentence with a 2-second mid-pause (expect either an
  `UtteranceEnd` flush split on the pause, or a `silence` flush — both
  are valid behaviour, depending on which trigger fires first).

If the resampler logs a build line:
```
[INFO  pipeline_stt_stdin] mic: building resampler for 48000 Hz × 1 ch
```
that confirms the negotiated format. PipeWire usually serves 48 kHz; the
resampler downsamples to the 16 kHz Deepgram wants.

### Tuning the buffer

The defaults in `TranscriptBufferConfig` are starting points (see
DESIGN §4.4). When you observe behaviour you don't like during real
use:

| Symptom | What to change |
|---|---|
| Sentences split mid-thought when you pause to think | Raise `silence_flush_ms` (default 1500). |
| Sentences run on for too long before flushing | Lower `max_chars_before_flush` (default 240) or `silence_flush_ms`. |
| Short replies like "yes." flush as a fragment | Raise `min_chars_for_punct_flush` (default 30) — though this is rarely the problem; the silence flush handles it. |

These will be config-driven from `~/.config/realtime-translation/config.toml`
once the app is wired up at Stage 8. For Stage 4 they're hard-coded
defaults; edit them in `crates/pipeline/src/transcript.rs`.

### Common issues at Stage 4

| Symptom | Cause | Fix |
|---|---|---|
| `DEEPGRAM_API_KEY not set` | Missing or unreadable `.env` | Confirm the file exists at the project root and contains `DEEPGRAM_API_KEY=...`. |
| `ws error: HTTP error: 401` | Invalid API key | Generate a new key at https://console.deepgram.com. |
| All `PARTIAL` and zero `FINALISED` | The buffer never sees `is_final=true` — typically because the audio is silence or pure tones (Deepgram won't finalise empty transcripts) | Speak louder / closer; verify with `pw-capture-wav` that the file has signal. |
| `FLUSHED (punctuation)` fires too eagerly on every short utterance | `min_chars_for_punct_flush` too low | Raise the threshold (default 30 should be fine for most speech). |
| No flush at end of `--wav` run | Bug — should always trail with a manual flush on EOF | Check `pipeline-stt-stdin` log for `wav: streaming complete` followed by `Deepgram: ws closed`; the manual flush is appended in the WS task. |

---

## Stage 5 — DeepL translation

**Purpose:** wire DeepL HTTP translation into the `pipeline-stt-stdin` binary
so that every `FLUSHED` event is immediately translated and printed as a
`TRANSLATED` line. Verifies the rolling context buffer keeps sentence meaning
coherent across thinking pauses.

### Setup

You need a DeepL API key. Put it in `.env` alongside the Deepgram key:

```
DEEPL_API_KEY=...
```

Free-tier keys end in `:fx` and use `api-free.deepl.com`; paid keys use
`api.deepl.com`. The binary auto-detects this from the key suffix.

If `DEEPL_API_KEY` is absent the binary falls back to Stage 4 mode (STT
only) and logs `translation disabled`. This lets you run either stage with
the same binary.

### Flags reference

| Flag | Default | Description |
|---|---|---|
| `--target-lang CODE` | `EN` | DeepL target language (e.g. `DE`, `NL`, `IT`, `ES`) |
| `--source-lang CODE` | *(auto-detect)* | Pin DeepL source language; omit to let DeepL detect |
| `--context N` | `5` | How many prior sentences to send as DeepL context |
| `--language CODE` | *(auto-detect)* | Deepgram STT language; omit for `multi` mode |

### Test 1 — wav replay with translation

```
# Record a clip with 2–3 sentences first (if you don't have one already):
cargo run --bin pw-capture-wav -- /tmp/utt.wav --secs 15

# Replay — source auto-detected, translate to English (default):
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav

# Translate German recording to English explicitly:
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav --source-lang DE --target-lang EN

# Translate to Dutch:
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav --target-lang NL
```

**Expected output shape (German recording → EN):**

```
[INFO ...] Deepgram model=nova-3 language=auto-detect (multi); buffer punct>=30 max>=240 silence=1500ms
[INFO ...] DeepL auto → EN (latency_optimized); context window = 5 sentences
[  2334 ms] PARTIAL    Abends
[  4988 ms] PARTIAL    Abends nach der Arbeit treffe ich mich oft mit Freunden.
[  6173 ms] FINALISED  Abends nach der Arbeit treffe ich mich oft mit Freunden.
[  6173 ms] FLUSHED    (punctuation)  Abends nach der Arbeit treffe ich mich oft mit Freunden.
[  6350 ms] TRANSLATED [→ EN]  In the evenings after work, I often meet up with friends.
[  9971 ms] FINALISED  Am Wochenende kann ich gut abschalten und entspannen.
[  9971 ms] FLUSHED    (punctuation)  Am Wochenende kann ich gut abschalten und entspannen.
[ 10053 ms] TRANSLATED [→ EN]  At the weekend I can really switch off and relax.
```

**What to check:**

1. `TRANSLATED` appears within ~100–200ms of `FLUSHED` — that's DeepL's
   `latency_optimized` RTT. If it takes >500ms check your network.
2. Both Deepgram and DeepL auto-detect the source language — you should
   not need `--source-lang` or `--language` for the common case.
3. The second sentence translation benefits from the context of the first —
   pronouns and topic references resolve better than they would in isolation.
4. `PARTIAL` and `FINALISED` lines continue to arrive while DeepL is running
   — translation does not block the STT event stream.

### Test 2 — live mic with translation

```
# Auto-detect everything, translate to English (default):
cargo run --bin pipeline-stt-stdin -- --mic --secs 30

# Speak German, get Dutch translation:
cargo run --bin pipeline-stt-stdin -- --mic --secs 30 --target-lang NL

# Narrow context window for faster speakers:
cargo run --bin pipeline-stt-stdin -- --mic --secs 30 --context 3
```

Speak a few sentences with natural pauses. After each sentence boundary
(punctuation flush or silence flush) a `TRANSLATED` line should arrive
within ~200ms.

### Context buffer tuning

The context window (default 5) is the number of prior *source* sentences
passed to DeepL alongside the current one. DeepL uses them to resolve
pronouns and maintain topic consistency — the context characters are not
billed. Tune via `--context N`:

- Increase if translations lose thread across long pauses.
- Decrease if you notice translation RTT climbing (more context = slightly
  larger request).

### Common issues at Stage 5

| Symptom | Cause | Fix |
|---|---|---|
| `translation disabled` log | `DEEPL_API_KEY` not in env or `.env` | Add it to `.env` at the project root |
| `DeepL HTTP 403` | Wrong key or key not activated | Check the key at deepl.com; free keys end in `:fx` |
| `DeepL HTTP 456` | Monthly character quota exhausted | Free tier = 500k chars/month; upgrade or wait for reset |
| `TRANSLATED` never appears | DeepL call hanging | Check endpoint reachable: `curl -s https://api-free.deepl.com/v2/translate` |
| Source not detected correctly | DeepL mis-detecting short sentences | Add `--source-lang DE` (or whichever language) to pin it |
| Translation quality poor for a language pair | `latency_optimized` weaker for some pairs | Edit `model_type` to `prefer_quality_optimized` in `crates/pipeline/src/deepl.rs` |

---

---

## Stage 6 — ElevenLabs TTS playback

**Purpose:** wire ElevenLabs streaming TTS into the same `pipeline-stt-stdin`
binary so that every translation is spoken aloud via your cloned voice.
Audio is routed to a PipeWire sink — the system default for testing, or
`translator_virtmic_sink` to feed a meeting client.

Stage 6 activates automatically when both `ELEVENLABS_API_KEY` and `VOICE_ID`
are present in `.env` alongside `DEEPGRAM_API_KEY` and `DEEPL_API_KEY`.

### Setup

Add to `.env`:

```
ELEVENLABS_API_KEY=...
VOICE_ID=...
```

`VOICE_ID` is the ID of your Instant Voice Clone from the ElevenLabs
dashboard (Voices → your clone → copy ID).

### Test 1 — wav replay, full chain

```
# Replay your German test recording — should hear EN translation spoken back:
cargo run --bin pipeline-stt-stdin -- --wav /tmp/utt.wav --target-lang EN
```

**Expected log/output:**

```
[INFO ...] Deepgram model=nova-3 language=auto-detect (multi); ...
[INFO ...] DeepL auto → EN (latency_optimized); context=5 sentences
[INFO ...] ElevenLabs model=eleven_flash_v2_5 format=pcm_44100 → playback 44100Hz
[INFO ...] ElevenLabs: connected
[  6173 ms] FLUSHED    (punctuation)  Abends nach der Arbeit ...
[  6350 ms] TRANSLATED [→ EN]  In the evenings after work, I often meet up with friends.
[INFO ...] tts playback negotiated: rate=44100 channels=1
            ← you hear the translation spoken in your cloned voice here
```

**What to check:**

1. You hear audio from your default sink within ~300–500ms of `TRANSLATED`
   appearing (Deepgram → DeepL → ElevenLabs TTFB).
2. The voice sounds like your clone — if it sounds like a generic EL voice,
   double-check `VOICE_ID` is the correct clone ID.
3. Multiple sentences play sequentially without gaps — the ring buffer
   and the `flush:true` signal per utterance ensure EL generates audio
   immediately after each translation.

### Test 2 — live mic, full chain

```
cargo run --bin pipeline-stt-stdin -- --mic --secs 60 --target-lang DE
```

Speak naturally in your source language. After each sentence pause you should
hear the translation spoken back within roughly 600–1000ms (endpointing +
DeepL + ElevenLabs TTFB).

### Test 3 — route to the virtual microphone

This is the real meeting use-case: your translated voice goes into the
meeting as if it were your mic.

```
# Virtmic must be installed first (Stage 3). Then:
cargo run --bin pipeline-stt-stdin -- --mic --secs 60 \
    --sink translator_virtmic_sink --target-lang DE
```

In another terminal, confirm the virtmic source is receiving signal:

```
cargo run --bin pw-capture-wav -- /tmp/check-virtmic.wav \
    --node translator_virtmic_source --secs 5
ffmpeg -hide_banner -nostats -i /tmp/check-virtmic.wav \
    -af volumedetect -f null /dev/null 2>&1 | grep max_volume
# Expect: max_volume well above -90 dB when speech is flowing
```

Or just open a browser, pick "Translator Virtual Mic" as the mic, and
confirm the other end hears your translated voice.

### Latency budget

| Stage | Typical latency |
|---|---|
| Speech end → Deepgram `speech_final` | 10–300ms |
| Deepgram final transcript | <300ms |
| DeepL `latency_optimized` | ~100–150ms |
| ElevenLabs Flash v2.5 TTFB | ~150–300ms |
| **Total: speech end → first audio byte** | **~600–1000ms** |

Tune `endpointing_ms` in `DeepgramConfig::with_detect_language` (currently
300ms) lower for snappier response at the cost of cutting trailing words.

### Common issues at Stage 6

| Symptom | Cause | Fix |
|---|---|---|
| `ELEVENLABS_API_KEY / VOICE_ID not set — TTS disabled` | Missing env var | Add both to `.env` |
| `ElevenLabs: connecting` log but no audio | WS connected but EL returned an error | Check `ERROR` lines — 401 = bad key, 422 = bad voice_id |
| Audio plays but sounds like a generic voice | `VOICE_ID` wrong | Copy the exact ID from EL dashboard → Voices |
| Audio crackles or drops out | Ring buffer too small or PCM bridge lagging | Ring is 2s at 44.1kHz — if EL is slow, it'll underrun; acceptable for now |
| `--sink translator_virtmic_sink` but no signal on source | Virtmic not installed | Re-run Stage 3 Part 1 |
| Audio plays to wrong speaker | Default sink reassigned | `pactl info \| grep "Default Sink"`; use `--sink NODE_NAME` to pin it |

---

## What's not yet tested (because it doesn't exist yet)

| Stage | Binary | What it'll do |
|---|---|---|
| 7 | (no new binary) | Track 2: capture meeting audio → STT → translate → print subtitles |
| 8 | `translator` | The user-facing app — egui UI, both tracks, control window + subtitle overlay |

This file gets a new section per stage as we build them.

---

## Quick reference: useful commands while testing

```
# What does PipeWire see right now?
pw-cli ls Node | grep -E "node\.name|media\.class|node\.description"

# What does the pulse compatibility shim see?
pactl info
pactl list short sources
pactl list short sinks

# What's the current state of every node?
wpctl status

# Watch nodes appear/disappear in real time:
pw-mon

# Reload the user-level PipeWire stack (use after editing config files):
systemctl --user restart pipewire wireplumber pipewire-pulse
```
