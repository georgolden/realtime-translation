//! pw-capture-wav — record N seconds from a PipeWire node to a wav file.
//!
//! Usage:
//!   pw-capture-wav OUT.wav            # capture from default source for 5s
//!   pw-capture-wav OUT.wav --secs 10
//!   pw-capture-wav OUT.wav --node 45              # capture node id 45 directly
//!   pw-capture-wav OUT.wav --sink-monitor 69      # capture what sink 69 is playing
//!
//! Pair with `pw-list-nodes` to find the ids.

use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use audio_os::{capture_for_duration, CaptureTarget};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let opts = parse_args()?;

    log::info!(
        "capturing for {:.1}s to {} (target: {:?})",
        opts.secs,
        opts.out.display(),
        opts.target,
    );

    // The wav writer is opened lazily once we know the negotiated format.
    let writer: Rc<RefCell<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>> =
        Rc::new(RefCell::new(None));
    let total_samples = Rc::new(RefCell::new(0u64));

    {
        let writer = writer.clone();
        let total = total_samples.clone();
        let out_path = opts.out.clone();

        capture_for_duration(opts.target, Duration::from_secs_f32(opts.secs), move |samples, fmt| {
            // Open the file the first time we see the negotiated format.
            let mut w = writer.borrow_mut();
            if w.is_none() {
                let spec = hound::WavSpec {
                    channels:        fmt.channels,
                    sample_rate:     fmt.sample_rate,
                    bits_per_sample: 32,
                    sample_format:   hound::SampleFormat::Float,
                };
                match hound::WavWriter::create(&out_path, spec) {
                    Ok(new_w) => *w = Some(new_w),
                    Err(e) => {
                        log::error!("failed to open {}: {e}", out_path.display());
                        return;
                    }
                }
            }
            // Append samples — interleaved f32, hound writes them as-is.
            if let Some(w) = w.as_mut() {
                for &s in samples {
                    if let Err(e) = w.write_sample(s) {
                        log::warn!("wav write failed: {e}");
                        break;
                    }
                }
                *total.borrow_mut() += samples.len() as u64;
            }
        })?;
    }

    // Finalise the wav file.
    if let Some(w) = writer.borrow_mut().take() {
        w.finalize()?;
    }

    let n = *total_samples.borrow();
    log::info!("wrote {} samples → {}", n, opts.out.display());
    if n == 0 {
        log::warn!("zero samples captured — is the source producing audio?");
    }
    Ok(())
}

#[derive(Debug)]
struct Opts {
    out:    PathBuf,
    secs:   f32,
    target: CaptureTarget,
}

fn parse_args() -> anyhow::Result<Opts> {
    let mut args = env::args().skip(1);
    let out = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing OUT.wav; see source for usage"))?
        .into();
    let mut secs = 5.0f32;
    let mut target = CaptureTarget::Default;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--secs" => {
                secs = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--secs needs a number"))?
                    .parse()?;
            }
            "--node" => {
                let id: u32 = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--node needs an id"))?
                    .parse()?;
                target = CaptureTarget::Node(id);
            }
            "--sink-monitor" => {
                let id: u32 = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sink-monitor needs an id"))?
                    .parse()?;
                target = CaptureTarget::SinkMonitor(id);
            }
            other => anyhow::bail!("unknown flag: {other}"),
        }
    }
    Ok(Opts { out, secs, target })
}
