//! audio-os — PipeWire integration layer.
//!
//! Stage 1: enumeration only. `list_nodes()` connects to the PipeWire daemon,
//! does one synchronous round-trip, and returns the snapshot of audio-related
//! nodes (sources, sinks, monitors).

mod nodes;

pub use nodes::{list_nodes, MediaClass, NodeInfo};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioOsError {
    #[error("pipewire error: {0}")]
    Pipewire(#[from] pipewire::Error),
}
