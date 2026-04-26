//! ui — egui front-end for the realtime translator (Stage 8).
//!
//! Module layout:
//!   config.rs    — AppConfig: load from .env + config.toml
//!   transcript.rs — TranscriptLog: timestamped session log file
//!   track.rs     — single-track pipeline runner (Outgoing / Incoming)
//!   session.rs   — SessionHandle: starts and owns two parallel tracks
//!   state.rs     — UiState: pure data model, no egui imports
//!   app.rs       — TranslatorApp: eframe App, control window + overlay

mod app;
mod config;
mod session;
mod state;
mod track;
mod transcript;

pub use app::TranslatorApp;
pub use config::AppConfig;
