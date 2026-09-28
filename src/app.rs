use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::fs;
use std::path::{Path, PathBuf};

pub struct NotesApp {
    current_file_path: Option<PathBuf>,
    editor_text: String,
    commonmark_cache: CommonMarkCache,
    file_tree: FileNode,
}

struct FileNode {
    path: PathBuf,
    is_dir: bool,
    children: Option<Vec<FileNode>>,
}

impl FileNode {
    fn new(path: PathBuf) -> Self {
        let is_dir = path.is_dir();
        let mut children = None;
        if is_dir {
            if let Ok(entries) = fs::read_dir(&path) {
                let mut c = Vec::new();
                for entry in entries.flatten() {
                    c.push(FileNode::new(entry.path()));
                }
                // Sort directories first, then by name
                c.sort_by(|a, b| {
                    b.is_dir.cmp(&a.is_dir).then_with(|| a.path.file_name().cmp(&b.path.file_name()))
                });
                children = Some(c);
            }
        }
        Self {
            path,
            is_dir,
            children,
        }
    }
}

impl NotesApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let workspace_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let file_tree = FileNode::new(workspace_dir.clone());

        Self {
            current_file_path: None,
            editor_text: String::new(),
            commonmark_cache: CommonMarkCache::default(),
            file_tree,
        }
    }

    fn open_file(&mut self, path: &Path) {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            let ext = ext.to_lowercase();
            match ext.as_str() {
                "md" | "txt" | "csv" | "json" | "yaml" | "yml" | "xml" | "py" | "rs" | "c" | "cpp" | "h" | "js" | "ts" => {
                    if let Ok(content) = fs::read_to_string(path) {
                        self.current_file_path = Some(path.to_path_buf());
                        self.editor_text = content;
                    }
                }
                "pdf" | "docx" | "xlsx" | "pptx" | "exe" | "png" | "jpg" | "jpeg" => {
                    // MVP: open with OS default
                    let _ = open::that(path);
                }
                _ => {
                    // Try to open as text, or default to OS
                    if let Ok(content) = fs::read_to_string(path) {
                        self.current_file_path = Some(path.to_path_buf());
                        self.editor_text = content;
                    } else {
                        let _ = open::that(path);
                    }
                }
            }
        } else {
            // No extension, try as text
            if let Ok(content) = fs::read_to_string(path) {
                self.current_file_path = Some(path.to_path_buf());
                self.editor_text = content;
            }
        }
    }

    fn save_current_file(&self) {
        if let Some(path) = &self.current_file_path {
            let _ = fs::write(path, &self.editor_text);
        }
    }

    fn render_file_tree(&self, ui: &mut egui::Ui, node: &FileNode) -> Option<PathBuf> {
        let mut clicked_path = None;
        if node.is_dir {
            let name = node.path.file_name().unwrap_or_default().to_string_lossy();
            egui::CollapsingHeader::new(name)
                .default_open(true)
                .show(ui, |ui| {
                    if let Some(children) = &node.children {
                        for child in children {
                            if let Some(p) = self.render_file_tree(ui, child) {
                                clicked_path = Some(p);
                            }
                        }
                    }
                });
        } else {
            let name = node.path.file_name().unwrap_or_default().to_string_lossy();
            if ui.selectable_label(self.current_file_path.as_ref() == Some(&node.path), name).clicked() {
                clicked_path = Some(node.path.clone());
            }
        }
        clicked_path
    }
}

impl eframe::App for NotesApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut file_to_open = None;

        egui::SidePanel::left("file_explorer_panel")
            .resizable(true)
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.heading("Explorer");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(path) = self.render_file_tree(ui, &self.file_tree) {
                        file_to_open = Some(path);
                    }
                });
            });

        if let Some(path) = file_to_open {
            self.open_file(&path);
        }

        // Right panel for preview, if it's a markdown file
        let mut show_preview = false;
        if let Some(path) = &self.current_file_path {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext.to_lowercase() == "md" {
                    show_preview = true;
                }
            }
        }

        if show_preview {
            egui::SidePanel::right("preview_panel")
                .resizable(true)
                .default_width(300.0)
                .show(ctx, |ui| {
                    ui.heading("Preview");
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new("viewer")
                            .show(ui, &mut self.commonmark_cache, &self.editor_text);
                    });
                });
        }

        // Central panel for editing
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(path) = &self.current_file_path {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                ui.heading(name);
                ui.separator();

                let response = egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut self.editor_text)
                                .font(egui::TextStyle::Monospace)
                                .code_editor()
                        )
                    });

                if response.inner.changed() {
                    // Auto-save logic
                    self.save_current_file();
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a file from the explorer.");
                });
            }
        });
    }
}
