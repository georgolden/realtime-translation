//! translator — Stage 8 user-facing app.
//!
//! Starts a multi-thread tokio runtime for pipeline tasks, then hands
//! control to eframe (native thread) running the egui event loop.

use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let cfg = ui::AppConfig::load();

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?,
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Realtime Translator")
            .with_inner_size(egui::Vec2::new(640.0, 720.0))
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "Realtime Translator",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::TranslatorApp::new(cc, cfg, rt.clone())))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}
