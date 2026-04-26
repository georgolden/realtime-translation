//! SessionHandle — owns both parallel tracks for the duration of one session.
//!
//! Starting a session:
//!   1. Creates a TranscriptLog file.
//!   2. Spawns Track 1 (mic → STT → translate → TTS → virtmic).
//!   3. Optionally spawns Track 2 (sink-monitor → STT → translate → subtitles).
//!   4. Returns a SessionHandle the UI uses to read events and stop the session.
//!
//! Stopping:
//!   - Sets the shared `stop` AtomicBool → both capture threads exit within 50 ms.
//!   - Dropping the Deepgram handles closes the WS connections.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::track::{track_configs_from_app, spawn_track};
use crate::transcript::TranscriptLog;

// ── Public types ───────────────────────────────────────────────────────────

/// Merged event from either track.
pub use crate::track::TrackEvent as SessionEvent;

pub struct SessionHandle {
    pub event_rx: mpsc::Receiver<SessionEvent>,
    stop:         Arc<AtomicBool>,
    pub log_path: std::path::PathBuf,
}

impl SessionHandle {
    /// Signal both tracks to stop. Non-blocking — capture threads exit within ~50 ms.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

// ── Start session ──────────────────────────────────────────────────────────

/// Start a new session. Returns a `SessionHandle` or an error if the log
/// file could not be created.
pub fn start_session(
    cfg:            &AppConfig,
    mic_node:       Option<u32>,
    sink_node:      Option<u32>,
    t1_target_lang: &str,
    t2_target_lang: &str,
    rt:             Arc<tokio::runtime::Runtime>,
) -> anyhow::Result<SessionHandle> {
    let log = TranscriptLog::open()?;
    let log_path = log.path.clone();

    let stop = Arc::new(AtomicBool::new(false));
    let (merged_tx, merged_rx) = mpsc::channel::<SessionEvent>(512);

    let (t1_cfg, t2_cfg) =
        track_configs_from_app(cfg, mic_node, sink_node, t1_target_lang, t2_target_lang);

    // Track 1 — always started.
    let mut t1_rx = spawn_track(t1_cfg, stop.clone(), log.clone(), rt.clone());
    let tx1 = merged_tx.clone();
    rt.spawn(async move {
        while let Some(evt) = t1_rx.recv().await {
            if tx1.send(evt).await.is_err() { break; }
        }
    });

    // Track 2 — optional.
    if let Some(t2_cfg) = t2_cfg {
        let mut t2_rx = spawn_track(t2_cfg, stop.clone(), log.clone(), rt.clone());
        let tx2 = merged_tx.clone();
        rt.spawn(async move {
            while let Some(evt) = t2_rx.recv().await {
                if tx2.send(evt).await.is_err() { break; }
            }
        });
    }

    Ok(SessionHandle { event_rx: merged_rx, stop, log_path })
}
