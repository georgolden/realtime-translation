//! TranscriptLog — append-only session log written to disk.
//!
//! Opens a timestamped file in `~/.local/share/realtime-translation/sessions/`
//! at session start and writes one line per event.
//!
//! Format:
//!   [2026-04-26T15:30:01Z] [MIC]   PARTIAL  hello there
//!   [2026-04-26T15:30:02Z] [MIC]   SOURCE   hello there friend.
//!   [2026-04-26T15:30:02Z] [MIC]   DE       Hallo da, Freund.
//!   [2026-04-26T15:30:05Z] [AUDIO] PARTIAL  gut morgen
//!   [2026-04-26T15:30:06Z] [AUDIO] SOURCE   guten Morgen, wie geht es Ihnen?
//!   [2026-04-26T15:30:07Z] [AUDIO] EN       Good morning, how are you?
//!
//! TRACK: MIC = Track 1 (outgoing), AUDIO = Track 2 (incoming)
//! TYPE:  PARTIAL | SOURCE (flushed source before translation) | <LANG_CODE>

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::config::sessions_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTrack {
    Mic,   // Track 1 — outgoing (user's microphone)
    Audio, // Track 2 — incoming (browser / system audio)
}

impl LogTrack {
    fn label(self) -> &'static str {
        match self {
            LogTrack::Mic   => "MIC  ",
            LogTrack::Audio => "AUDIO",
        }
    }
}

/// A session transcript log file. Cheap to clone — writes are serialized
/// through an inner `Mutex<BufWriter>`.
#[derive(Clone)]
pub struct TranscriptLog {
    inner: Arc<Mutex<Inner>>,
    pub path: PathBuf,
}

struct Inner {
    writer: BufWriter<File>,
}

impl TranscriptLog {
    /// Open a new timestamped log file. Creates the sessions directory if needed.
    pub fn open() -> anyhow::Result<Self> {
        let dir = sessions_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine sessions directory"))?;
        fs::create_dir_all(&dir)?;

        let ts = timestamp_filename();
        let path = dir.join(format!("{ts}.log"));
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);

        log::info!("TranscriptLog: writing session to {}", path.display());
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner { writer })),
            path,
        })
    }

    /// Write a PARTIAL line (live in-flight transcript).
    pub fn log_partial(&self, track: LogTrack, text: &str) {
        self.write_line(track, "PARTIAL", text);
    }

    /// Write a SOURCE line (flushed source text, before translation).
    pub fn log_source(&self, track: LogTrack, text: &str) {
        self.write_line(track, "SOURCE ", text);
    }

    /// Write a translated line. `lang` is the target language code, e.g. `"DE"`.
    pub fn log_translated(&self, track: LogTrack, lang: &str, text: &str) {
        self.write_line(track, lang, text);
    }

    fn write_line(&self, track: LogTrack, kind: &str, text: &str) {
        let ts = iso8601_now();
        let line = format!("[{ts}] [{}] {:<8} {}\n", track.label(), kind, text);
        if let Ok(mut inner) = self.inner.lock() {
            let _ = inner.writer.write_all(line.as_bytes());
            let _ = inner.writer.flush();
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn iso8601_now() -> String {
    // Format: 2026-04-26T15:30:01Z  (second precision, UTC)
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    unix_to_iso8601(secs)
}

fn timestamp_filename() -> String {
    // Filename-safe ISO8601: 2026-04-26_15-30-01
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d, h, mi, s) = unix_decompose(secs);
    format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}")
}

fn unix_to_iso8601(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = unix_decompose(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

// Minimal UTC decomposition — no chrono dep.
fn unix_decompose(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s  = (secs % 60) as u32;
    let mi = ((secs / 60) % 60) as u32;
    let h  = ((secs / 3600) % 24) as u32;
    let days = secs / 86400;

    // Gregorian calendar from day count (days since 1970-01-01)
    let z  = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = doy - (153 * mp + 2) / 5 + 1;
    let mo  = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if mo <= 2 { y + 1 } else { y };

    (y as u32, mo as u32, d as u32, h, mi, s)
}
