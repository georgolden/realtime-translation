//! pw-list-nodes — print every audio node visible in PipeWire.
//!
//! Mirrors a subset of `pactl list short sources` / `... sinks`, but
//! exercises the path inside `audio-os::list_nodes()` that the rest of
//! the app relies on.

use audio_os::{list_nodes, MediaClass};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut nodes = list_nodes()?;
    // Group by class for readability; ids stay as PipeWire reports them.
    nodes.sort_by_key(|n| (class_order(n.media_class), n.id));

    println!("{:>5}  {:<14}  {:<48}  {}", "id", "class", "name", "description");
    println!("{}", "-".repeat(120));
    for n in &nodes {
        println!(
            "{:>5}  {:<14}  {:<48}  {}",
            n.id,
            class_label(n.media_class),
            truncate(&n.name, 48),
            n.description,
        );
    }
    println!();
    println!("{} audio nodes", nodes.len());
    Ok(())
}

fn class_order(c: MediaClass) -> u8 {
    match c {
        MediaClass::Source => 0,
        MediaClass::Sink => 1,
        MediaClass::StreamOutput => 2,
        MediaClass::StreamInput => 3,
        MediaClass::Other => 4,
    }
}

fn class_label(c: MediaClass) -> &'static str {
    match c {
        MediaClass::Source => "source",
        MediaClass::Sink => "sink",
        MediaClass::StreamOutput => "stream-out",
        MediaClass::StreamInput => "stream-in",
        MediaClass::Other => "other",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
