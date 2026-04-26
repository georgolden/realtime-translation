//! audio-os — PipeWire integration layer.
//!
//! Stage 1: enumeration (`list_nodes`).
//! Stage 2: capture (`capture_for_duration`).
//! Stage 3: playback (`play_for_duration`) — used to write into the
//!          translator virtual mic.

mod capture;
mod nodes;
mod playback;

pub use capture::{capture_for_duration, AudioFormat, CaptureTarget};
pub use nodes::{list_nodes, MediaClass, NodeInfo};
pub use playback::{play_for_duration, PlaybackFormat, PlaybackTarget};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioOsError {
    #[error("pipewire error: {0}")]
    Pipewire(#[from] pipewire::Error),
    #[error("failed to build EnumFormat param")]
    FormatBuild,
    #[error("failed to arm capture stop timer")]
    TimerArm,
}
