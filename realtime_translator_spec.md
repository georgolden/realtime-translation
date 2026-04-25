# Real-Time Meeting Translator — Implementation Spec

## Concept

Two-track streaming pipeline for live meeting translation using your cloned voice.

- **Outgoing (you):** your mic → STT → translate → TTS with your voice clone → meeting audio out
- **Incoming (others):** meeting audio → STT → translate → subtitles displayed to you

No end-to-end voice-to-voice product supports voice cloning at acceptable latency today.
This cascade pipeline is the best viable architecture as of April 2026.

---

## Stack

| Component | Service | Model/Config |
|---|---|---|
| STT (streaming) | Deepgram | `nova-3` |
| Translation | DeepL Text API | `latency_optimized` |
| TTS + Voice Clone | ElevenLabs | `eleven_flash_v2_5` + IVC |

---

## Track 1 — Outgoing (your voice, cloned)

### Step 1: Deepgram Streaming STT

WebSocket endpoint: `wss://api.deepgram.com/v1/listen`

Auth: `Authorization: Token <DEEPGRAM_API_KEY>`

Key params:
```
model=nova-3
language=en
interim_results=true
endpointing=300          # ms of silence before speech_final=true (tune to taste)
utterance_end_ms=1000    # fallback: fires UtteranceEnd if no words for 1s (requires interim_results=true)
smart_format=true
punctuate=true
```

Response fields to watch:
- `is_final: false` → interim result, buffer but don't act yet
- `is_final: true` → Deepgram is confident, safe to translate
- `speech_final: true` → speaker paused (endpointing fired), trigger downstream immediately
- `UtteranceEnd` message → hard end-of-speech signal even in noisy environments

Docs:
- Streaming: https://developers.deepgram.com/docs/live-streaming-audio
- Endpointing: https://developers.deepgram.com/docs/endpointing
- Interim Results: https://developers.deepgram.com/docs/interim-results
- UtteranceEnd: https://developers.deepgram.com/docs/understanding-end-of-speech-detection

**Trigger translation on `speech_final: true` or `UtteranceEnd`, not on every `is_final`.**

---

### Step 2: DeepL Text Translation

Endpoint: `POST https://api.deepl.com/v2/translate`

Auth: `Authorization: DeepL-Auth-Key <DEEPL_API_KEY>`

Payload:
```json
{
  "text": ["<transcript chunk>"],
  "source_lang": "EN",
  "target_lang": "DE",
  "model_type": "latency_optimized"
}
```

Supported target languages for your use case: `DE`, `NL`, `IT`, `ES`
For German: use `DE` (Hochdeutsch — no dialect-specific target).

`model_type` options (from docs):
- `latency_optimized` — fastest, use this for real-time
- `quality_optimized` — best quality, higher latency
- `prefer_quality_optimized` — quality with fallback

Docs:
- Translate text: https://developers.deepl.com/api-reference/translate
- Supported languages: https://developers.deepl.com/docs/resources/supported-languages
- Python SDK: https://github.com/DeepLcom/deepl-python
- Node SDK: https://github.com/DeepLcom/deepl-node

Free tier: 500,000 characters/month.

---

### Step 3: ElevenLabs TTS WebSocket (streaming, voice clone)

WebSocket endpoint: `wss://api.elevenlabs.io/v1/text-to-speech/<voice_id>/stream-input`

Model: `eleven_flash_v2_5` (~75ms inference, 32 languages including DE, NL, IT, ES)

**European users:** use `api-global-preview.elevenlabs.io` for 150–200ms TTFB instead of US default.
Docs: https://elevenlabs.io/docs/best-practices/latency-optimization

Init message (send on connect):
```json
{
  "text": " ",
  "model_id": "eleven_flash_v2_5",
  "voice_settings": {
    "stability": 0.5,
    "similarity_boost": 0.8
  },
  "generation_config": {
    "chunk_length_schedule": [120, 160, 250]
  }
}
```

Stream translated text chunks:
```json
{ "text": "Hallo, wie geht es Ihnen? ", "flush": false }
```

Force flush at end of utterance:
```json
{ "text": "", "flush": true }
```

Keep connection alive between utterances:
```json
{ "text": " " }
```
(Connection auto-closes after 20s of inactivity.)

Audio comes back as base64-encoded MP3 chunks — decode and pipe to audio output.

Voice clone latency note (from docs): Instant Voice Clones (IVC) are slightly slower than
default voices. Professional Voice Clones (PVC) latency is being actively optimized for Flash v2.5.
Use IVC for lowest latency; PVC for highest quality.

Docs:
- WebSocket TTS guide: https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts
- WebSocket API ref: https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-stream-input
- Models: https://elevenlabs.io/docs/overview/models
- Latency optimization: https://elevenlabs.io/docs/best-practices/latency-optimization
- Voice cloning (IVC): https://elevenlabs.io/docs/eleven-creative/voices/voice-cloning/instant-voice-cloning

---

## Track 2 — Incoming (others → your subtitles)

Same Deepgram WebSocket setup, but:
- `language=de` (or `nl`, `it`, `es` depending on speaker — consider `detect_language=true`)
- Transcripts feed into DeepL `target_lang: EN`
- Display as rolling subtitles in UI — no TTS needed

For German you can optionally skip translation entirely and show raw German transcript.

---

## Latency Budget (Track 1, rough estimates)

| Stage | Latency |
|---|---|
| You finish speaking → Deepgram `speech_final` | ~10–300ms (endpointing) |
| Deepgram final transcript | <300ms TTFT |
| DeepL `latency_optimized` | ~100–150ms |
| ElevenLabs Flash v2.5 (EU infra) | ~150–200ms TTFB |
| **Total: speech end → first audio byte** | **~600–800ms** |

Tune `endpointing` lower (e.g. 150ms) for snappier response at the cost of cutting off trailing words.

---

## Voice Clone Setup

ElevenLabs Instant Voice Clone:
1. Dashboard → Voices → + → Instant Voice Clone
2. Upload 1–2 min of clean audio (no background noise, no reverb, consistent tone)
3. Record consent phrase when prompted
4. Save → get `voice_id` for API calls

Docs: https://elevenlabs.io/docs/eleven-creative/voices/voice-cloning/instant-voice-cloning

---

## API Keys Needed

- `DEEPGRAM_API_KEY` — https://console.deepgram.com ($200 free credit)
- `DEEPL_API_KEY` — https://www.deepl.com/en/pro-api (500k chars/month free)
- `ELEVENLABS_API_KEY` + `VOICE_ID` — https://elevenlabs.io (Creator plan ~$22/mo for IVC + Flash)
