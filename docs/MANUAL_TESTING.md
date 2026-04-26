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

## What's not yet tested (because it doesn't exist yet)

| Stage | Binary | What it'll do |
|---|---|---|
| 4 | `pipeline-stt-stdin` | Stream a wav into Deepgram; print transcripts |
| 5 | (extends Stage 4) | Add DeepL output to the same binary |
| 6 | (extends Stage 4 + audio-os virtmic) | Full Track 1, real meeting test |
| 7 | (no new binary) | Track 2 prints translated subtitles to stdout |
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
