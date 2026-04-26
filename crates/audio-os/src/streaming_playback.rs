//! Streaming playback into a PipeWire sink.
//!
//! Unlike `play_for_duration` (which drives audio via a fill callback for a
//! fixed duration), `StreamingPlayer` accepts PCM frames pushed from an async
//! producer (e.g. an ElevenLabs WebSocket task) and plays them in real time.
//!
//! Architecture:
//!   async task          →  ringbuf SPSC  →  PipeWire RT thread
//!   (push_pcm / finish)                     (on_process callback)
//!
//! The PipeWire main loop runs on a dedicated OS thread spawned by
//! `StreamingPlayer::spawn`. The caller pushes f32 frames via `push_pcm` and
//! calls `finish()` to drain + stop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};

use crate::{AudioOsError, PlaybackFormat, PlaybackTarget};

/// Handle used by the producer (async side) to feed frames into the player
/// and signal when it's done.
pub struct StreamingPlayerHandle {
    producer:  ringbuf::HeapProd<f32>,
    done_flag: Arc<AtomicBool>,
}

impl StreamingPlayerHandle {
    /// Push interleaved f32 frames. Non-blocking — drops on overflow (ring
    /// is large enough that this shouldn't happen at TTS speeds).
    pub fn push_pcm(&mut self, samples: &[f32]) {
        let pushed = self.producer.push_slice(samples);
        if pushed < samples.len() {
            log::warn!(
                "streaming_playback: ring full, dropped {} samples",
                samples.len() - pushed
            );
        }
    }

    /// Signal that no more audio is coming. The PipeWire thread will drain
    /// the ring and then stop the main loop.
    pub fn finish(self) {
        self.done_flag.store(true, Ordering::Release);
        // `producer` is dropped here — that's fine, the consumer checks
        // `done_flag` to know when to stop.
    }
}

/// Spawn a PipeWire playback loop on a blocking OS thread.
///
/// Returns a `StreamingPlayerHandle` for the producer side and a
/// `JoinHandle` for the PipeWire thread (so the caller can wait for
/// drain to finish).
///
/// `ring_capacity`: number of f32 samples in the ring buffer. 48000 * 2 * 2
/// (2 s of stereo 48 kHz) is a generous default.
pub fn spawn_streaming_player(
    target:        PlaybackTarget,
    format:        PlaybackFormat,
    ring_capacity: usize,
) -> (StreamingPlayerHandle, std::thread::JoinHandle<Result<(), AudioOsError>>) {
    let rb = HeapRb::<f32>::new(ring_capacity);
    let (producer, consumer) = rb.split();
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_flag_clone = done_flag.clone();

    let handle = StreamingPlayerHandle { producer, done_flag };

    let join = std::thread::spawn(move || {
        run_playback_loop(target, format, consumer, done_flag_clone)
    });

    (handle, join)
}

fn run_playback_loop(
    target:    PlaybackTarget,
    format:    PlaybackFormat,
    mut consumer: ringbuf::HeapCons<f32>,
    done_flag: Arc<AtomicBool>,
) -> Result<(), AudioOsError> {
    use pipewire as pw;
    use pw::spa;
    use spa::pod::Pod;
    use std::cell::Cell;
    use std::rc::Rc;

    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context  = pw::context::ContextRc::new(&mainloop, None)?;
    let core     = context.connect_rc(None)?;

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE     => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE     => "Music",
        *pw::keys::APP_NAME       => "realtime-translation",
        *pw::keys::AUDIO_CHANNELS => format.channels.to_string(),
    };
    if let PlaybackTarget::NodeName(ref name) = target {
        props.insert(*pw::keys::TARGET_OBJECT, name.as_str());
    }

    let stream = pw::stream::StreamBox::new(&core, "translator-tts-playback", props)?;

    let negotiated = Rc::new(Cell::new(None::<crate::AudioFormat>));
    // Whether the ring is drained AND done_flag is set — used to quit the loop.
    let should_quit = Rc::new(Cell::new(false));

    let _listener = {
        let neg_pc  = negotiated.clone();
        let neg_pr  = negotiated.clone();
        let sq      = should_quit.clone();
        let stride_bytes = (format.channels as usize) * std::mem::size_of::<f32>();

        stream
            .add_local_listener_with_user_data(())
            .param_changed(move |_stream, _user, id, param| {
                let Some(param) = param else { return; };
                if id != pw::spa::param::ParamType::Format.as_raw() { return; }
                let Ok((mt, ms)) = spa::param::format_utils::parse_format(param) else { return; };
                if mt != spa::param::format::MediaType::Audio
                    || ms != spa::param::format::MediaSubtype::Raw { return; }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err() { return; }
                neg_pc.set(Some(crate::AudioFormat {
                    sample_rate: info.rate(),
                    channels:    info.channels() as u16,
                }));
                log::info!("tts playback negotiated: rate={} channels={}", info.rate(), info.channels());
            })
            .process(move |stream, _user| {
                let Some(_fmt) = neg_pr.get() else { return; };
                let Some(mut buffer) = stream.dequeue_buffer() else { return; };

                let datas = buffer.datas_mut();
                if datas.is_empty() { return; }
                let data = &mut datas[0];
                let Some(slice) = data.data() else { return; };

                let max_bytes   = slice.len();
                let max_samples = max_bytes / std::mem::size_of::<f32>();
                if max_samples == 0 { return; }

                let f32_slice: &mut [f32] =
                    bytemuck::cast_slice_mut(&mut slice[..max_samples * std::mem::size_of::<f32>()]);

                let available = consumer.occupied_len();
                let to_read   = available.min(max_samples);
                let read      = consumer.pop_slice(&mut f32_slice[..to_read]);

                // Silence anything we didn't fill.
                for s in &mut f32_slice[read..] { *s = 0.0; }

                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride_bytes as i32;
                *chunk.size_mut()   = (max_samples * std::mem::size_of::<f32>()) as u32;

                // Quit once the producer is done and the ring is empty.
                if done_flag.load(Ordering::Acquire) && consumer.is_empty() {
                    sq.set(true);
                }
            })
            .register()?
    };

    // Build EnumFormat pod.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    audio_info.set_rate(format.sample_rate);
    audio_info.set_channels(format.channels as u32);

    let mut position = [0u32; spa::param::audio::MAX_CHANNELS];
    match format.channels {
        1 => position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO,
        2 => { position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL; position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR; }
        _ => {}
    }
    audio_info.set_position(position);

    let bytes: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_:      pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id:         pw::spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        }),
    )
    .map_err(|_| AudioOsError::FormatBuild)?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or(AudioOsError::FormatBuild)?];

    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Poll until done. The process callback sets `should_quit` when the ring
    // is drained and the producer signalled finish.
    let ml = mainloop.clone();
    let sq_check = should_quit.clone();

    // Add a 10ms polling timer to check the quit flag — the process callback
    // can't directly quit the mainloop (it runs on the RT thread).
    let timer = mainloop.loop_().add_timer(move |_| {
        if sq_check.get() {
            ml.quit();
        }
    });
    timer
        .update_timer(
            Some(Duration::from_millis(10)),
            Some(Duration::from_millis(10)),
        )
        .into_sync_result()
        .map_err(|_| AudioOsError::TimerArm)?;

    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}
