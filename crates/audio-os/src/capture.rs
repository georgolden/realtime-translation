//! Audio capture from a PipeWire node.
//!
//! Two variants:
//!   - `capture_for_duration` — runs for a fixed wall-clock duration (dev binaries, tests).
//!   - `capture_indefinite`   — runs until an `Arc<AtomicBool>` stop flag is set (app).
//!
//! Both are synchronous: the caller's thread runs the PipeWire mainloop.
//! The `on_frame` callback fires on that thread between frames — must not block.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;

use crate::AudioOsError;

/// Negotiated audio format reported by PipeWire. Sample format is always
/// `f32` little-endian — that's what we ask for in the EnumFormat param,
/// and PipeWire will resample/convert as needed.
#[derive(Debug, Clone, Copy)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels:    u16,
}

#[derive(Debug, Clone)]
pub enum CaptureTarget {
    /// Use the default source PipeWire picks for us.
    Default,
    /// A specific node id (mic, virtual source, sink).
    Node(u32),
    /// A sink — captures the monitor (= what's playing on that sink).
    /// Equivalent to `Node(id)` plus `stream.capture.sink=true`.
    SinkMonitor(u32),
    /// Monitor of whatever is the current default sink (no node id needed).
    DefaultSinkMonitor,
}

/// Capture from `target` for `duration`, calling `on_frame` with
/// interleaved f32 samples.
///
/// `on_frame(samples, format)` is invoked on the loop thread (not RT,
/// but a hot path — keep it cheap; ringbuf push is ideal). `format` is
/// the negotiated rate / channel count.
///
/// Returns once `duration` elapses, after a clean stream disconnect.
pub fn capture_for_duration<F>(
    target: CaptureTarget,
    duration: Duration,
    on_frame: F,
) -> Result<(), AudioOsError>
where
    F: FnMut(&[f32], AudioFormat) + 'static,
{
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context  = pw::context::ContextRc::new(&mainloop, None)?;
    let core     = context.connect_rc(None)?;

    // Stream properties: standard "Capture" media role, plus optional
    // monitor flag for the SinkMonitor case.
    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE     => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE     => "Music",
        *pw::keys::APP_NAME       => "realtime-translation",
    };
    if matches!(target, CaptureTarget::SinkMonitor(_) | CaptureTarget::DefaultSinkMonitor) {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }

    let stream = pw::stream::StreamBox::new(&core, "audio-capture", props)?;

    // Shared format slot — written by `param_changed`, read by `process`.
    let format = Rc::new(Cell::new(None::<AudioFormat>));

    // The user callback lives in a RefCell because both the param_changed
    // and process closures need to share access to it through the user-data
    // mechanism (we keep them in two listener callbacks instead and route
    // the shared state via `Rc<Cell<…>>` and `Rc<RefCell<…>>`).
    let on_frame: Rc<RefCell<F>> = Rc::new(RefCell::new(on_frame));

    let _listener = {
        let format_pc = format.clone();
        let format_pr = format.clone();
        let on_frame  = on_frame.clone();

        stream
            .add_local_listener_with_user_data(())
            .param_changed(move |_stream, _user, id, param| {
                let Some(param) = param else { return; };
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                    return;
                };
                if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                    return;
                }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err() {
                    return;
                }
                format_pc.set(Some(AudioFormat {
                    sample_rate: info.rate(),
                    channels:    info.channels() as u16,
                }));
                log::info!(
                    "negotiated format: rate={} channels={}",
                    info.rate(),
                    info.channels(),
                );
            })
            .process(move |stream, _user| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    log::warn!("dequeue_buffer returned None (out of buffers)");
                    return;
                };
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }
                // PipeWire delivers interleaved f32 in datas[0] for our
                // negotiated format. A multi-plane layout would split
                // channels across datas[i]; we'd ask for that explicitly
                // and we don't, so this is safe.
                let Some(fmt) = format_pr.get() else { return; };
                let chunk = datas[0].chunk();
                let n_bytes = chunk.size() as usize;
                if n_bytes == 0 {
                    return;
                }
                let Some(raw) = datas[0].data() else { return; };
                let raw = &raw[..n_bytes.min(raw.len())];
                // PipeWire writes F32LE (little-endian). On x86_64 native
                // == LE, so `cast_slice` is fine. If we ever build for BE
                // we'd byte-swap here; not a v1 concern.
                let samples: &[f32] = bytemuck::cast_slice(raw);
                (on_frame.borrow_mut())(samples, fmt);
            })
            .register()?
    };

    // Build the EnumFormat param: F32LE, native rate/channels (we leave
    // those at default to accept whatever the source serves).
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_:      pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id:         pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let bytes: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|_| AudioOsError::FormatBuild)?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or(AudioOsError::FormatBuild)?];

    // Resolve target → (Option<node_id>) for connect().
    let target_id = match target {
        CaptureTarget::Default | CaptureTarget::DefaultSinkMonitor => None,
        CaptureTarget::Node(id) | CaptureTarget::SinkMonitor(id)   => Some(id),
    };

    stream.connect(
        spa::utils::Direction::Input,
        target_id,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Schedule a one-shot quit after `duration`.
    // We use the loop's timer source: callback fires on the loop thread
    // and calls `mainloop.quit()`.
    let stop_ml = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_expirations| {
        stop_ml.quit();
    });
    timer
        .update_timer(Some(duration), None)
        .into_sync_result()
        .map_err(|_| AudioOsError::TimerArm)?;

    mainloop.run();

    // Best-effort disconnect; ignore errors at shutdown.
    let _ = stream.disconnect();
    Ok(())
}

/// Capture from `target` indefinitely, calling `on_frame` until `stop` is
/// set to `true`. The mainloop wakes every 50 ms to check the flag — this
/// avoids the `TryFromIntError` panic that occurs when a timer is armed with
/// `Duration::from_secs(u64::MAX)`.
///
/// Call from a dedicated OS thread (e.g. `std::thread::spawn` or
/// `tokio::task::spawn_blocking`). Block until `stop` is set, then return.
pub fn capture_indefinite<F>(
    target: CaptureTarget,
    stop:   Arc<AtomicBool>,
    on_frame: F,
) -> Result<(), AudioOsError>
where
    F: FnMut(&[f32], AudioFormat) + 'static,
{
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context  = pw::context::ContextRc::new(&mainloop, None)?;
    let core     = context.connect_rc(None)?;

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE     => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE     => "Music",
        *pw::keys::APP_NAME       => "realtime-translation",
    };
    if matches!(target, CaptureTarget::SinkMonitor(_) | CaptureTarget::DefaultSinkMonitor) {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }

    let stream = pw::stream::StreamBox::new(&core, "audio-capture-indefinite", props)?;

    let format: Rc<Cell<Option<AudioFormat>>> = Rc::new(Cell::new(None));
    let on_frame: Rc<RefCell<F>> = Rc::new(RefCell::new(on_frame));

    let _listener = {
        let format_pc = format.clone();
        let format_pr = format.clone();
        let on_frame  = on_frame.clone();

        stream
            .add_local_listener_with_user_data(())
            .param_changed(move |_stream, _user, id, param| {
                let Some(param) = param else { return; };
                if id != pw::spa::param::ParamType::Format.as_raw() { return; }
                let Ok((media_type, media_subtype)) =
                    spa::param::format_utils::parse_format(param) else { return; };
                if media_type != spa::param::format::MediaType::Audio
                    || media_subtype != spa::param::format::MediaSubtype::Raw { return; }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err() { return; }
                format_pc.set(Some(AudioFormat {
                    sample_rate: info.rate(),
                    channels:    info.channels() as u16,
                }));
                log::info!(
                    "capture_indefinite: negotiated format: rate={} channels={}",
                    info.rate(), info.channels(),
                );
            })
            .process(move |stream, _user| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return; };
                let datas = buffer.datas_mut();
                if datas.is_empty() { return; }
                let Some(fmt) = format_pr.get() else { return; };
                let chunk  = datas[0].chunk();
                let n_bytes = chunk.size() as usize;
                if n_bytes == 0 { return; }
                let Some(raw) = datas[0].data() else { return; };
                let raw = &raw[..n_bytes.min(raw.len())];
                let samples: &[f32] = bytemuck::cast_slice(raw);
                (on_frame.borrow_mut())(samples, fmt);
            })
            .register()?
    };

    // EnumFormat param — same as capture_for_duration.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_:      pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id:         pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let bytes: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|_| AudioOsError::FormatBuild)?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or(AudioOsError::FormatBuild)?];

    let target_id = match target {
        CaptureTarget::Default | CaptureTarget::DefaultSinkMonitor => None,
        CaptureTarget::Node(id) | CaptureTarget::SinkMonitor(id)   => Some(id),
    };

    stream.connect(
        spa::utils::Direction::Input,
        target_id,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Poll every 50 ms to check the stop flag. 50 ms is well within PipeWire's
    // representable timer range and adds negligible shutdown latency.
    let stop_ml = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        if stop.load(Ordering::Acquire) {
            stop_ml.quit();
        }
    });
    timer
        .update_timer(
            Some(Duration::from_millis(50)),
            Some(Duration::from_millis(50)),
        )
        .into_sync_result()
        .map_err(|_| AudioOsError::TimerArm)?;

    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}
