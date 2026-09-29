#![windows_subsystem = "windows"]

mod app;

use eframe::egui;

fn main() -> eframe::Result<()> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let icon =
        image::load_from_memory_with_format(include_bytes!("../icon.ico"), image::ImageFormat::Ico)
            .expect("The bundled application icon must be valid")
            .into_rgba8();
    let (width, height) = icon.dimensions();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Selva — Notes and connections")
            .with_icon(egui::IconData {
                rgba: icon.into_raw(),
                width,
                height,
            })
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Selva — Note e connessioni",
        options,
        Box::new(|cc| {
            // Optional: configure egui to look nicer
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Box::new(app::NotesApp::new(cc))
        }),
    )
}
