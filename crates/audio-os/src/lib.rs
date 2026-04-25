//! audio-os — PipeWire integration layer.
//!
//! Stage 1: enumeration (`list_nodes`).
//! Stage 2: capture (`capture_for_duration`).

mod capture;
mod nodes;

pub use capture::{capture_for_duration, AudioFormat, CaptureTarget};
pub use nodes::{list_nodes, MediaClass, NodeInfo};

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
