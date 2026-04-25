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

## What's not yet tested (because it doesn't exist yet)

| Stage | Binary | What it'll do |
|---|---|---|
| 3 | `pw-virtmic-tone` | Play a 440 Hz tone into the translator virtual mic; verify in Firefox's mic picker |
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
