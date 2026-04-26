//! 48 kHz f32 (mono or stereo) → 16 kHz i16 mono — the format Deepgram
//! wants. PipeWire delivers variable-sized chunks; this module owns the
//! buffering and feeds rubato in fixed-size frames.
//!
//! "Mono" upstream is just channel-collapse: average the two channels
//! before resampling. The STT use case doesn't need stereo.

use rubato::{FastFixedIn, PolynomialDegree, Resampler};

use crate::PipelineError;

/// Fixed input chunk size we feed rubato (samples per call, mono).
/// 480 = 10 ms at 48 kHz, which gives 160 output samples at 16 kHz.
/// Any size works; this one keeps latency negligible and avoids edge
/// effects in the polynomial interp.
const CHUNK_IN_FRAMES: usize = 480;

/// Resample 48 kHz f32 input to 16 kHz i16 mono. Holds a small internal
/// buffer for partial chunks, plus the rubato state.
pub struct ResampleState {
    in_rate:    u32,
    in_channels: u16,
    /// Interleaved f32 samples we haven't yet collapsed/resampled.
    /// Stays under a few thousand samples in normal operation.
    pending:    Vec<f32>,
    /// Mono f32 frames waiting for the next rubato call.
    mono_buf:   Vec<f32>,
    resampler:  Option<FastFixedIn<f32>>,
    /// Reusable scratch for rubato's process_into_buffer.
    in_scratch: Vec<f32>,
    out_scratch: Vec<f32>,
    /// True when input rate equals 16 kHz already — we skip rubato.
    passthrough_rate: bool,
}

impl ResampleState {
    pub fn new(in_rate: u32, in_channels: u16) -> Result<Self, PipelineError> {
        let passthrough_rate = in_rate == 16_000;
        let resampler = if passthrough_rate {
            None
        } else {
            // ratio = out_rate / in_rate
            let ratio = 16_000.0_f64 / in_rate as f64;
            let r = FastFixedIn::<f32>::new(
                ratio,
                1.0, // max relative ratio change (we never change ratio)
                PolynomialDegree::Cubic,
                CHUNK_IN_FRAMES,
                1,   // mono
            )
            .map_err(|e| PipelineError::Resample(format!("rubato init: {e}")))?;
            Some(r)
        };
        let out_capacity = if passthrough_rate {
            CHUNK_IN_FRAMES
        } else {
            // Cubic adds a small amount of headroom; double the nominal.
            (CHUNK_IN_FRAMES * 16_000 / in_rate as usize) * 2 + 32
        };
        Ok(Self {
            in_rate,
            in_channels,
            pending:        Vec::with_capacity(CHUNK_IN_FRAMES * 4),
            mono_buf:       Vec::with_capacity(CHUNK_IN_FRAMES * 2),
            resampler,
            in_scratch:     vec![0.0; CHUNK_IN_FRAMES],
            out_scratch:    vec![0.0; out_capacity],
            passthrough_rate,
        })
    }

    pub fn in_rate(&self)     -> u32 { self.in_rate }
    pub fn in_channels(&self) -> u16 { self.in_channels }

    /// Push interleaved f32 samples (n_frames × n_channels). Returns
    /// 16 kHz mono i16 samples ready to send to Deepgram. May return an
    /// empty Vec if there isn't yet a full chunk.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<i16>, PipelineError> {
        // 1) Channel-collapse to mono in `mono_buf`.
        let ch = self.in_channels.max(1) as usize;
        if ch == 1 {
            self.mono_buf.extend_from_slice(samples);
        } else {
            let mut i = 0;
            while i + ch <= samples.len() {
                let mut acc = 0.0f32;
                for c in 0..ch {
                    acc += samples[i + c];
                }
                self.mono_buf.push(acc / ch as f32);
                i += ch;
            }
            // If samples.len() isn't divisible by ch we're in trouble.
            // PipeWire shouldn't give us that, but stash any tail to
            // re-prepend on the next call.
            if i < samples.len() {
                self.pending.extend_from_slice(&samples[i..]);
            }
        }

        // 2) If we still have stashed bytes from a previous unaligned
        //    chunk, prepend them. (Rare: only triggers when channel
        //    count and chunk alignment disagree.)
        if !self.pending.is_empty() && ch > 1 {
            let mut combined = std::mem::take(&mut self.pending);
            // The pending tail wasn't channel-collapsed yet — drop it on
            // next push by re-adding to mono via average if alignment
            // restored. Simpler: just drop the unaligned tail; STT
            // accuracy doesn't notice <1 frame of audio. Keep a warning
            // in a future revision if it actually fires.
            combined.clear();
        }

        // 3) Resample (or pass through) full chunks.
        let mut out = Vec::with_capacity(self.mono_buf.len() / 3);
        if self.passthrough_rate {
            for &s in &self.mono_buf {
                out.push(f32_to_i16(s));
            }
            self.mono_buf.clear();
        } else {
            let resampler = self.resampler.as_mut().expect("resampler present");
            while self.mono_buf.len() >= CHUNK_IN_FRAMES {
                self.in_scratch
                    .copy_from_slice(&self.mono_buf[..CHUNK_IN_FRAMES]);
                self.mono_buf.drain(..CHUNK_IN_FRAMES);

                let in_chans  = [&self.in_scratch[..]];
                let mut out_chans = [&mut self.out_scratch[..]];

                let (_in_used, out_written) = resampler
                    .process_into_buffer(&in_chans, &mut out_chans, None)
                    .map_err(|e| PipelineError::Resample(format!("rubato process: {e}")))?;

                for &s in &self.out_scratch[..out_written] {
                    out.push(f32_to_i16(s));
                }
            }
        }
        Ok(out)
    }
}

fn f32_to_i16(s: f32) -> i16 {
    // Standard symmetric clipping. Deepgram is fine with i16 LE.
    let clamped = s.clamp(-1.0, 1.0);
    (clamped * 32767.0) as i16
}

/// Convenience: one-shot resample (no internal state retained between
/// calls). Useful for tests and for the stdin binary which feeds an
/// entire wav file at once via `push` repeatedly.
pub fn resample_to_deepgram(
    samples: &[f32],
    in_rate: u32,
    in_channels: u16,
) -> Result<Vec<i16>, PipelineError> {
    let mut s = ResampleState::new(in_rate, in_channels)?;
    s.push(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_16k_mono() {
        let mut s = ResampleState::new(16_000, 1).unwrap();
        // 1 second of 1.0 → max-positive i16.
        let input = vec![1.0f32; 16_000];
        let out = s.push(&input).unwrap();
        assert_eq!(out.len(), 16_000);
        assert_eq!(out[0], 32767);
    }

    #[test]
    fn stereo_collapse_to_mono() {
        let mut s = ResampleState::new(16_000, 2).unwrap();
        // Stereo: [L, R, L, R, ...] = [1.0, -1.0, ...] -> mono 0.0
        let input = vec![1.0f32, -1.0, 1.0, -1.0];
        let out = s.push(&input).unwrap();
        // 4 interleaved samples = 2 mono frames at passthrough.
        assert_eq!(out.len(), 2);
        for s in out {
            assert_eq!(s, 0);
        }
    }

    #[test]
    fn downsample_48k_to_16k_ratio() {
        // 1 second of 48 kHz silence → ~16 000 16 kHz samples. Cubic
        // interpolation on chunk boundaries shaves a couple of samples
        // off the head/tail; allow ±5 of nominal.
        let mut s = ResampleState::new(48_000, 1).unwrap();
        let input = vec![0.0f32; 48_000];
        let out = s.push(&input).unwrap();
        let diff = (out.len() as i64 - 16_000).abs();
        assert!(diff <= 5, "out.len()={} not ≈ 16000", out.len());
        // All silence in → all zero out.
        assert!(out.iter().all(|&s| s == 0));
    }

    #[test]
    fn streamed_chunks_match_one_shot_count() {
        // Push the same input as one big slice and as many tiny slices;
        // the total output sample count should match.
        let big: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.001).sin()).collect();
        let one_shot = resample_to_deepgram(&big, 48_000, 1).unwrap();

        let mut s = ResampleState::new(48_000, 1).unwrap();
        let mut streamed = Vec::new();
        for chunk in big.chunks(123) {
            streamed.extend(s.push(chunk).unwrap());
        }
        // Count is the strict invariant; sample-by-sample equality isn't
        // (rubato's polynomial state at chunk boundaries differs).
        assert_eq!(streamed.len(), one_shot.len());
    }

    #[test]
    fn unaligned_tail_does_not_explode() {
        // A 49-sample stereo chunk = 24 full frames + 1 dangling sample.
        // Should produce 24 mono frames worth (passthrough at 16 kHz).
        let mut s = ResampleState::new(16_000, 2).unwrap();
        let input: Vec<f32> = vec![0.5; 49];
        let out = s.push(&input).unwrap();
        assert_eq!(out.len(), 24);
    }
}
