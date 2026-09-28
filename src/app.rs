use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
enum DisplayMode {
    ViewOnly,
    EditAndPreview,
    EditOnly,
}

enum FileAction {
    None,
    Open(PathBuf),
    Delete(PathBuf),
    CreateNote(PathBuf),
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
    force_expand_collapse: Option<bool>,
    create_note_target_dir: Option<PathBuf>,
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
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut workspace_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut display_mode = DisplayMode::EditAndPreview;

        if let Some(storage) = cc.storage {
            if let Some(saved_dir) = eframe::get_value::<PathBuf>(storage, "workspace_dir") {
                if saved_dir.exists() {
                    workspace_dir = saved_dir;
                }
            }
            if let Some(saved_mode) = eframe::get_value::<DisplayMode>(storage, "display_mode") {
                display_mode = saved_mode;
            }
        }

        let file_tree = FileNode::new(workspace_dir.clone());

        Self {
            workspace_dir,
            current_file_path: None,
            editor_text: String::new(),
            commonmark_cache: CommonMarkCache::default(),
            file_tree,
            is_creating_note: false,
            new_note_name: String::new(),
            display_mode,
            force_expand_collapse: None,
            create_note_target_dir: None,
        }
    }

    fn open_file(&mut self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::env::set_current_dir(parent);
        }

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

    fn render_file_tree(&self, ui: &mut egui::Ui, node: &FileNode) -> FileAction {
        let mut action = FileAction::None;
        let name = node.path.file_name().unwrap_or_default().to_string_lossy();

        if node.is_dir {
            let mut header = egui::CollapsingHeader::new(name).default_open(true);
            if let Some(force) = self.force_expand_collapse {
                header = header.open(Some(force));
            }

            let response = header.show(ui, |ui| {
                if let Some(children) = &node.children {
                    for child in children {
                        let child_action = self.render_file_tree(ui, child);
                        if !matches!(child_action, FileAction::None) {
                            action = child_action;
                        }
                    }
                }
            });

            response.header_response.context_menu(|ui| {
                if ui.button("➕ New Note").clicked() {
                    action = FileAction::CreateNote(node.path.clone());
                    ui.close_menu();
                }
                if ui.button("📂 Open in Explorer").clicked() {
                    let _ = open::that(&node.path);
                    ui.close_menu();
                }
            });
        } else {
            let res = ui.selectable_label(self.current_file_path.as_ref() == Some(&node.path), name);
            if res.clicked() {
                action = FileAction::Open(node.path.clone());
            }
            res.context_menu(|ui| {
                if ui.button("🗑 Delete").clicked() {
                    action = FileAction::Delete(node.path.clone());
                    ui.close_menu();
                }
                if ui.button("📂 Open in Explorer").clicked() {
                    let _ = open::that(node.path.parent().unwrap_or(&node.path));
                    ui.close_menu();
                }
            });
        }
        action
    }
}

impl eframe::App for NotesApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "workspace_dir", &self.workspace_dir);
        eframe::set_value(storage, "display_mode", &self.display_mode);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut action = FileAction::None;

        egui::SidePanel::left("file_explorer_panel")
            .resizable(true)
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Explorer");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🔽").on_hover_text("Collapse All").clicked() {
                            self.force_expand_collapse = Some(false);
                        }
                        if ui.button("▶").on_hover_text("Expand All").clicked() {
                            self.force_expand_collapse = Some(true);
                        }
                        if ui.button("🔄").on_hover_text("Refresh").clicked() {
                            self.file_tree = FileNode::new(self.workspace_dir.clone());
                        }
                        if ui.button("➕").on_hover_text("New Note").clicked() {
                            self.is_creating_note = true;
                            self.new_note_name = String::from("Untitled.md");
                            self.create_note_target_dir = Some(self.workspace_dir.clone());
                        }
                        if ui.button("📂").on_hover_text("Open Folder").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.workspace_dir = folder.clone();
                                self.file_tree = FileNode::new(folder);
                                self.current_file_path = None;
                                self.editor_text = String::new();
                            }
                        }
                    });
                });
                
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    ui.selectable_value(&mut self.display_mode, DisplayMode::ViewOnly, "👁 View");
                    ui.selectable_value(&mut self.display_mode, DisplayMode::EditAndPreview, "📖 Split");
                    ui.selectable_value(&mut self.display_mode, DisplayMode::EditOnly, "✏ Edit");
                });
                
                ui.separator();

                if self.is_creating_note {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.new_note_name);
                        if ui.button("Create").clicked() {
                            let target_dir = self.create_note_target_dir.clone().unwrap_or(self.workspace_dir.clone());
                            let new_path = target_dir.join(&self.new_note_name);
                            if !new_path.exists() {
                                let _ = fs::write(&new_path, "");
                                // Refresh tree and open new file
                                self.file_tree = FileNode::new(self.workspace_dir.clone());
                                self.open_file(&new_path);
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
                    let tree_action = self.render_file_tree(ui, &self.file_tree);
                    if !matches!(tree_action, FileAction::None) {
                        action = tree_action;
                    }
                });
            });

        match action {
            FileAction::Open(path) => self.open_file(&path),
            FileAction::Delete(path) => {
                let _ = fs::remove_file(&path);
                self.file_tree = FileNode::new(self.workspace_dir.clone());
                if self.current_file_path == Some(path) {
                    self.current_file_path = None;
                    self.editor_text = String::new();
                }
            }
            FileAction::CreateNote(path) => {
                self.is_creating_note = true;
                self.new_note_name = String::from("Untitled.md");
                self.create_note_target_dir = Some(path);
            }
            FileAction::None => {}
        }
        
        // Reset force expand/collapse after one frame
        self.force_expand_collapse = None;

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
                
                ui.heading(name);
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
