#![windows_subsystem = "windows"]

mod app;
mod editor;

use eframe::egui;

fn window_icon(bytes: &[u8]) -> Option<egui::IconData> {
    // Some ICO files contain RGB PNGs, which the ICO decoder rejects.
    // Decode those PNG entries directly and add their alpha channel ourselves.
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Ico)
        .ok()
        .or_else(|| {
            if bytes.get(..4)? != [0, 0, 1, 0] {
                return None;
            }
            let count = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?);
            (0..usize::from(count))
                .filter_map(|index| {
                    let start = 6 + index * 16;
                    let entry = bytes.get(start..start + 16)?;
                    let size = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
                    let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
                    let png = bytes.get(offset..offset.checked_add(size)?)?;
                    image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()
                })
                .max_by_key(|image| u64::from(image.width()) * u64::from(image.height()))
        })?;
    let rgba = decoded.into_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn main() -> eframe::Result<()> {
    let mut logger = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    );
    // A GUI executable has no console: retain startup diagnostics for VMs.
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let directory = std::path::PathBuf::from(base).join("Selva").join("logs");
        if std::fs::create_dir_all(&directory).is_ok() {
            if let Ok(file) = std::fs::File::create(directory.join("startup.log")) {
                logger.target(env_logger::Target::Pipe(Box::new(file)));
            }
        }
    }
    logger.init();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Selva — Notes and connections")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([800.0, 600.0]);
    if let Some(icon) = window_icon(include_bytes!("../icon.ico")) {
        viewport = viewport.with_icon(icon);
    } else {
        log::warn!("Could not decode the window icon; using the default icon");
    }

    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    #[cfg(target_os = "windows")]
    {
        // DX12 also enumerates Windows' software adapter on Hyper-V/RDP.
        options.renderer = eframe::Renderer::Wgpu;
        options.wgpu_options.supported_backends = eframe::wgpu::Backends::DX12;
    }

    let result = eframe::run_native(
        "Selva — Note e connessioni",
        options,
        Box::new(|cc| {
            if let Some(state) = &cc.wgpu_render_state {
                let adapter = state.adapter.get_info();
                log::info!("Graphics adapter: {} ({:?}, {:?})", adapter.name, adapter.backend, adapter.device_type);
            }
            // Optional: configure egui to look nicer
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Box::new(app::NotesApp::new(cc))
        }),
    );
    if let Err(error) = &result {
        log::error!("Selva startup failed: {error}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_icon_decodes_for_the_window() {
        let icon = window_icon(include_bytes!("../icon.ico")).expect("Custom icon should decode");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }

    #[test]
    fn invalid_icons_do_not_panic() {
        for bytes in [&[][..], &[0, 0, 1, 0, 1, 0][..], b"invalid icon"] {
            assert!(window_icon(bytes).is_none());
        }
        let mut bytes = vec![0, 0, 1, 0, 1, 0];
        bytes.extend_from_slice(&[255; 16]);
        assert!(window_icon(&bytes).is_none());
    }
}
