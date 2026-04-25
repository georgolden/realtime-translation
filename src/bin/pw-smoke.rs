//! pw-smoke — minimum viable PipeWire connection.
//!
//! Verifies that the `pipewire` crate builds against the system libraries
//! and can connect to the running daemon. If this prints OK, Module 1
//! Risk A (fork to raw `pipewire-sys`) is closed.

use pipewire as pw;

fn main() -> Result<(), pw::Error> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let _core = context.connect_rc(None)?;

    println!("OK: connected to PipeWire from Rust");
    Ok(())
}
