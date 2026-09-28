use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(PartialEq)]
enum DisplayMode {
    ViewOnly,
    EditAndPreview,
    EditOnly,
}

pub struct NotesApp {
    workspace_dir: PathBuf,
    current_file_path: Option<PathBuf>,
    editor_text: String,
    commonmark_cache: CommonMarkCache,
    file_tree: FileNode,
    is_creating_note: bool,
    new_note_name: String,
    display_mode: DisplayMode,
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
            workspace_dir,
            current_file_path: None,
            editor_text: String::new(),
            commonmark_cache: CommonMarkCache::default(),
            file_tree,
            is_creating_note: false,
            new_note_name: String::new(),
            display_mode: DisplayMode::EditAndPreview,
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
                
                ui.horizontal(|ui| {
                    if ui.button("📂 Open Folder").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.workspace_dir = folder.clone();
                            self.file_tree = FileNode::new(folder);
                            self.current_file_path = None;
                            self.editor_text = String::new();
                        }
                    }
                });
                
                ui.horizontal(|ui| {
                    if ui.button("➕ New Note").clicked() {
                        self.is_creating_note = true;
                        self.new_note_name = String::from("Untitled.md");
                    }
                    if ui.button("🔄 Refresh").clicked() {
                        self.file_tree = FileNode::new(self.workspace_dir.clone());
                    }
                });

                if self.is_creating_note {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.new_note_name);
                        if ui.button("Create").clicked() {
                            let new_path = self.workspace_dir.join(&self.new_note_name);
                            if !new_path.exists() {
                                let _ = fs::write(&new_path, "");
                                // Refresh tree and open new file
                                self.file_tree = FileNode::new(self.workspace_dir.clone());
                                self.current_file_path = Some(new_path);
                                self.editor_text = String::new();
                            }
                            self.is_creating_note = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.is_creating_note = false;
                        }
                    });
                }

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

        // Determine if it's a markdown file
        let mut is_markdown = false;
        if let Some(path) = &self.current_file_path {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext.to_lowercase() == "md" {
                    is_markdown = true;
                }
            }
        }

        // Right panel for preview, ONLY in split mode
        let show_right_preview = is_markdown && self.display_mode == DisplayMode::EditAndPreview;

        if show_right_preview {
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

        // Central panel
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(path) = &self.current_file_path {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                
                ui.horizontal(|ui| {
                    ui.heading(name);
                    if is_markdown {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.selectable_value(&mut self.display_mode, DisplayMode::ViewOnly, "👁 View");
                            ui.selectable_value(&mut self.display_mode, DisplayMode::EditAndPreview, "📖 Split");
                            ui.selectable_value(&mut self.display_mode, DisplayMode::EditOnly, "✏ Edit");
                        });
                    }
                });
                
                ui.separator();

                if is_markdown && self.display_mode == DisplayMode::ViewOnly {
                    // Full screen preview
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new("central_viewer")
                            .show(ui, &mut self.commonmark_cache, &self.editor_text);
                    });
                } else {
                    // Editor
                    let previous_text_len = self.editor_text.chars().count();
                    
                    let output = egui::ScrollArea::vertical()
                        .show(ui, |ui| {
                            egui::TextEdit::multiline(&mut self.editor_text)
                                .font(egui::TextStyle::Monospace)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .show(ui)
                        }).inner;

                    if output.response.changed() {
                        let new_text_len = self.editor_text.chars().count();
                        // Auto-pairing logic
                        if new_text_len == previous_text_len + 1 {
                            if let Some(cursor_range) = output.cursor_range {
                                let cursor_idx = cursor_range.primary.ccursor.index;
                                if cursor_idx > 0 && cursor_idx <= new_text_len {
                                    let inserted_char = self.editor_text.chars().nth(cursor_idx - 1).unwrap();
                                    let pair = match inserted_char {
                                        '*' => Some('*'),
                                        '_' => Some('_'),
                                        '`' => Some('`'),
                                        '(' => Some(')'),
                                        '[' => Some(']'),
                                        '{' => Some('}'),
                                        '"' => Some('"'),
                                        '\'' => Some('\''),
                                        _ => None,
                                    };
                                    
                                    if let Some(p) = pair {
                                        self.editor_text.insert(cursor_idx, p);
                                    }
                                }
                            }
                        }
                        
                        self.save_current_file();
                    }
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a file from the explorer.");
                });
            }
        });
    }
}
