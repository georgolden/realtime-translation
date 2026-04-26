//! pw-virtmic-tone — play a 440 Hz sine to the translator virtual mic
//! (or another sink) for N seconds.
//!
//! Usage:
//!   pw-virtmic-tone                              # 5 s, default sink
//!   pw-virtmic-tone --secs 10
//!   pw-virtmic-tone --node-name translator_virtmic_sink
//!   pw-virtmic-tone --node-name translator_virtmic_sink --freq 880 --volume 0.3
//!
//! Sanity-test before installing the virtmic: omit `--node-name` to play to
//! the default sink — you should hear the tone in your headphones. Then
//! re-run with `--node-name translator_virtmic_sink` once the virtmic
//! config is installed (see docs/MANUAL_TESTING.md, Stage 3).

use std::env;
use std::f32::consts::PI;
use std::time::Duration;

use audio_os::{play_for_duration, PlaybackFormat, PlaybackTarget};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let opts = parse_args()?;

    let target = match opts.node_name.clone() {
        Some(n) => PlaybackTarget::NodeName(n),
        None    => PlaybackTarget::Default,
    };
    log::info!(
        "playing {:.1} Hz tone for {:.1}s (volume {:.2}) to {:?}",
        opts.freq, opts.secs, opts.volume, target,
    );

    let format = PlaybackFormat::stereo_48k();

    // Phase accumulator. f32 here is fine for sub-second runs; for hours of
    // playback a f64 phase prevents drift, but we don't run that long.
    let mut phase = 0.0f32;
    let two_pi = 2.0 * PI;

    play_for_duration(target, format, Duration::from_secs_f32(opts.secs), move |out, fmt| {
        let dphase = two_pi * opts.freq / fmt.sample_rate as f32;
        let channels = fmt.channels as usize;
        let frames   = out.len() / channels;
        for f in 0..frames {
            let s = phase.sin() * opts.volume;
            for c in 0..channels {
                out[f * channels + c] = s;
            }
            phase += dphase;
            if phase >= two_pi {
                phase -= two_pi;
            }
        }
        frames * channels
    })?;

    log::info!("done");
    Ok(())
}

#[derive(Debug)]
struct Opts {
    secs:      f32,
    freq:      f32,
    volume:    f32,
    node_name: Option<String>,
}

fn parse_args() -> anyhow::Result<Opts> {
    let mut args = env::args().skip(1);
    let mut o = Opts { secs: 5.0, freq: 440.0, volume: 0.2, node_name: None };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--secs"      => o.secs   = args.next().ok_or_else(|| anyhow::anyhow!("--secs needs a number"))?.parse()?,
            "--freq"      => o.freq   = args.next().ok_or_else(|| anyhow::anyhow!("--freq needs a number"))?.parse()?,
            "--volume"    => o.volume = args.next().ok_or_else(|| anyhow::anyhow!("--volume needs 0.0..1.0"))?.parse()?,
            "--node-name" => o.node_name = Some(args.next().ok_or_else(|| anyhow::anyhow!("--node-name needs a string"))?),
            other         => anyhow::bail!("unknown flag: {other}"),
        }
    }
    if !(0.0..=1.0).contains(&o.volume) {
        anyhow::bail!("--volume must be between 0.0 and 1.0");
    }
    Ok(o)
}
