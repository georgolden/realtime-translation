//! Live integration test against the running PipeWire daemon.
//!
//! Run with `cargo test -p audio-os --test list_nodes_live -- --ignored`.
//! Marked `#[ignore]` so the default `cargo test` stays hermetic.

use audio_os::{list_nodes, MediaClass};

#[test]
#[ignore = "requires a running PipeWire daemon"]
fn lists_at_least_one_audio_node() {
    let nodes = list_nodes().expect("list_nodes failed");
    assert!(
        !nodes.is_empty(),
        "PipeWire reported zero audio nodes — that's possible only on a system with no sound stack at all"
    );
}

#[test]
#[ignore = "requires a running PipeWire daemon"]
fn at_least_one_sink_present() {
    let nodes = list_nodes().expect("list_nodes failed");
    let has_sink = nodes
        .iter()
        .any(|n| matches!(n.media_class, MediaClass::Sink));
    assert!(
        has_sink,
        "expected at least one Audio/Sink node; got: {:#?}",
        nodes
    );
}
