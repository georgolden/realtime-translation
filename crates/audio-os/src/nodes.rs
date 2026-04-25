//! Node enumeration via the PipeWire registry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pipewire as pw;
use pw::types::ObjectType;

use crate::AudioOsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaClass {
    /// A real audio input — microphone, line-in, USB capture device.
    /// Also `Audio/Source/Virtual`, so the translator's own virtmic
    /// shows up here once installed.
    Source,
    /// An audio output. To capture *what an output is playing* (e.g.
    /// "all browser audio"), capture this node with the
    /// `stream.capture.sink` flag — PipeWire does not expose a
    /// separate monitor node, only monitor ports on the sink itself.
    Sink,
    /// A per-application playback stream — a single app currently
    /// playing audio. Useful for capturing only that app.
    StreamOutput,
    /// A per-application capture stream (an app reading from a mic).
    StreamInput,
    /// Something audio-related we don't have a category for.
    Other,
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: u32,
    /// Stable identifier (`node.name` in PipeWire).
    pub name: String,
    /// Human-readable label (`node.description`). Falls back to `name`
    /// when the node didn't set one.
    pub description: String,
    pub media_class: MediaClass,
}

/// Connect to PipeWire, do one synchronous registry round-trip, return
/// the snapshot of audio nodes.
///
/// Blocks the calling thread until the round-trip completes (typically a
/// few milliseconds against a local daemon). Not async — PipeWire's API
/// is callback-based and we drive a small mainloop for the duration of
/// this call.
pub fn list_nodes() -> Result<Vec<NodeInfo>, AudioOsError> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry()?;

    let nodes: Rc<RefCell<Vec<NodeInfo>>> = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(Cell::new(false));

    // Trigger the round-trip *before* registering the listener: the server's
    // reply is queued and only consumed once the loop runs, so there's no
    // race window.
    let pending = core.sync(0)?;

    let _core_listener = {
        let done = done.clone();
        let mainloop = mainloop.clone();
        core.add_listener_local()
            .done(move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done.set(true);
                    mainloop.quit();
                }
            })
            .register()
    };

    let _registry_listener = {
        let nodes = nodes.clone();
        registry
            .add_listener_local()
            .global(move |obj| {
                if obj.type_ != ObjectType::Node {
                    return;
                }
                let Some(props) = obj.props.as_ref() else {
                    return;
                };

                let media_class_raw = props.get("media.class").unwrap_or("");

                let media_class = classify(media_class_raw);
                if media_class == MediaClass::Other && !is_audio(media_class_raw) {
                    // Skip non-audio nodes (video, midi, etc.) entirely.
                    return;
                }

                let name = props
                    .get("node.name")
                    .unwrap_or("<unnamed>")
                    .to_string();
                let description = props
                    .get("node.description")
                    .map(str::to_string)
                    .unwrap_or_else(|| name.clone());

                nodes.borrow_mut().push(NodeInfo {
                    id: obj.id,
                    name,
                    description,
                    media_class,
                });
            })
            .register()
    };

    // Run the loop until the `done` callback flips the flag. The example
    // in vendor/pipewire-rs/pipewire/examples/roundtrip.rs uses the same
    // pattern.
    while !done.get() {
        mainloop.run();
    }

    Ok(nodes.take())
}

fn is_audio(media_class: &str) -> bool {
    media_class.starts_with("Audio/") || media_class.starts_with("Stream/")
}

fn classify(media_class: &str) -> MediaClass {
    match media_class {
        "Audio/Source" | "Audio/Source/Virtual" => MediaClass::Source,
        "Audio/Sink" => MediaClass::Sink,
        "Stream/Output/Audio" => MediaClass::StreamOutput,
        "Stream/Input/Audio" => MediaClass::StreamInput,
        _ => MediaClass::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_known_classes() {
        assert_eq!(classify("Audio/Source"), MediaClass::Source);
        assert_eq!(classify("Audio/Source/Virtual"), MediaClass::Source);
        assert_eq!(classify("Audio/Sink"), MediaClass::Sink);
        assert_eq!(classify("Stream/Output/Audio"), MediaClass::StreamOutput);
        assert_eq!(classify("Stream/Input/Audio"), MediaClass::StreamInput);
        assert_eq!(classify("Video/Source"), MediaClass::Other);
    }

    #[test]
    fn is_audio_filter() {
        assert!(is_audio("Audio/Sink"));
        assert!(is_audio("Stream/Output/Audio"));
        assert!(!is_audio("Video/Source"));
        assert!(!is_audio(""));
    }
}
