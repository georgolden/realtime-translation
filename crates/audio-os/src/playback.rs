//! Audio playback into a PipeWire sink.
//!
//! Synchronous, single-threaded — same shape as `capture_for_duration`.
//! Used by Stage 3's `pw-virtmic-tone` binary, and later by the
//! pipeline module to write ElevenLabs PCM into the translator virtmic.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use spa::pod::Pod;

use crate::capture::AudioFormat;
use crate::AudioOsError;

#[derive(Debug, Clone)]
pub enum PlaybackTarget {
    /// Connect to whatever the default sink is.
    Default,
    /// Connect by `node.name` (e.g. `translator_virtmic_sink`).
    /// Set as `PW_KEY_TARGET_OBJECT` on the stream so PipeWire routes us
    /// regardless of the node's id (which can change across restarts).
    NodeName(String),
}

/// Playback config — what format we want to send.
///
/// We *declare* this format up-front (unlike capture which accepts
/// whatever the source serves). PipeWire will resample if the sink's
/// native rate differs.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackFormat {
    pub sample_rate: u32,
    pub channels:    u16,
}

impl PlaybackFormat {
    pub fn stereo_48k() -> Self {
        Self { sample_rate: 48_000, channels: 2 }
    }
}

/// Play audio to `target` for `duration`. The `fill` callback is invoked
/// each time PipeWire asks for more data; it must write up to
/// `out.len()` interleaved f32 samples and return the number written.
///
/// Writing fewer samples than requested is fine — the rest of the
/// buffer will be silenced. Writing zero stops cleanly.
pub fn play_for_duration<F>(
    target: PlaybackTarget,
    format: PlaybackFormat,
    duration: Duration,
    fill: F,
) -> Result<(), AudioOsError>
where
    F: FnMut(&mut [f32], AudioFormat) -> usize + 'static,
{
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

    let stream = pw::stream::StreamBox::new(&core, "translator-playback", props)?;

    // Negotiated format — we asked for `format`, but PipeWire reports
    // back what was actually agreed (rate may differ if the sink is
    // pinned to a different rate).
    let negotiated = Rc::new(Cell::new(None::<AudioFormat>));
    let fill: Rc<RefCell<F>> = Rc::new(RefCell::new(fill));

    let _listener = {
        let neg_pc = negotiated.clone();
        let neg_pr = negotiated.clone();
        let fill   = fill.clone();
        let stride_bytes = (format.channels as usize) * std::mem::size_of::<f32>();

        stream
            .add_local_listener_with_user_data(())
            .param_changed(move |_stream, _user, id, param| {
                let Some(param) = param else { return; };
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) =
                    spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != spa::param::format::MediaType::Audio
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err() {
                    return;
                }
                neg_pc.set(Some(AudioFormat {
                    sample_rate: info.rate(),
                    channels:    info.channels() as u16,
                }));
                log::info!(
                    "playback negotiated format: rate={} channels={}",
                    info.rate(),
                    info.channels(),
                );
            })
            .process(move |stream, _user| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    log::warn!("dequeue_buffer returned None during playback");
                    return;
                };
                let Some(fmt) = neg_pr.get() else {
                    return;
                };

                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }
                let data = &mut datas[0];

                let Some(slice) = data.data() else { return; };
                let max_bytes  = slice.len();
                let max_frames = max_bytes / stride_bytes;
                if max_frames == 0 {
                    return;
                }

                // Reinterpret the byte buffer as f32 samples (LE on x86_64).
                let max_samples = max_frames * fmt.channels as usize;
                let f32_slice: &mut [f32] =
                    bytemuck::cast_slice_mut(&mut slice[..max_samples * std::mem::size_of::<f32>()]);

                let written_samples = (fill.borrow_mut())(f32_slice, fmt);
                let written_samples = written_samples.min(max_samples);
                let written_frames  = written_samples / fmt.channels as usize;

                // Silence the trailing portion the callback didn't fill.
                if written_samples < max_samples {
                    for s in &mut f32_slice[written_samples..] {
                        *s = 0.0;
                    }
                }

                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride_bytes as i32;
                // Always declare the full buffer as written — silence is fine,
                // PipeWire still needs a non-zero size for the stream to flow.
                *chunk.size_mut()   = (max_frames * stride_bytes) as u32;

                if written_frames == 0 {
                    log::trace!("fill produced zero frames — sending silence");
                }
            })
            .register()?
    };

    // Build EnumFormat — declare our preferred rate and channel count.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    audio_info.set_rate(format.sample_rate);
    audio_info.set_channels(format.channels as u32);

    // Channel positions: standard FL/FR for stereo, MONO for 1ch.
    let mut position = [0u32; spa::param::audio::MAX_CHANNELS];
    match format.channels {
        1 => position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO,
        2 => {
            position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
            position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
        }
        _ => {
            // Leave at default (UNKNOWN); PipeWire will pick a layout.
            log::warn!("unusual channel count {}; layout may be wrong", format.channels);
        }
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

    let stop_ml = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| stop_ml.quit());
    timer
        .update_timer(Some(duration), None)
        .into_sync_result()
        .map_err(|_| AudioOsError::TimerArm)?;

    mainloop.run();

    let _ = stream.disconnect();
    Ok(())
}
