use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

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
    CreateFolder(PathBuf),
    ShowInFileManager(PathBuf),
    MoveNote { source: PathBuf, folder: PathBuf },
}

struct DraggedNote {
    path: PathBuf,
    vault: PathBuf,
}

const EXPLORER_ROW_HEIGHT: f32 = 26.0;

fn explorer_row(ui: &mut egui::Ui, path: &Path, label: &str, selected: bool) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), EXPLORER_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(
        rect,
        egui::Id::new(("explorer_row", path)),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.has_focus() {
            ui.painter().rect(
                rect,
                visuals.rounding,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
            );
        }
        let galley = ui.painter().layout_no_wrap(
            label.to_owned(),
            egui::TextStyle::Button.resolve(ui.style()),
            visuals.text_color(),
        );
        let position = egui::pos2(
            rect.left() + ui.spacing().button_padding.x,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(rect))
            .galley(position, galley, visuals.text_color());
    }
    response
}

fn note_drag_source(response: egui::Response, path: &Path, vault: &Path) -> egui::Response {
    if !matches!(open_kind(path), OpenKind::Markdown | OpenKind::Text) {
        return response;
    }
    let response = response.interact(egui::Sense::click_and_drag());
    if response.drag_started_by(egui::PointerButton::Primary) {
        egui::DragAndDrop::set_payload(
            &response.ctx,
            DraggedNote {
                path: path.to_path_buf(),
                vault: vault.to_path_buf(),
            },
        );
    }
    response
}

// Return true after hovering long enough to expand a closed folder.
fn note_drop_target(
    ui: &egui::Ui,
    response: &egui::Response,
    folder: &Path,
    vault: &Path,
    action: &mut FileAction,
) -> bool {
    let timer_id = response.id.with("drop_hover");
    let Some(_) = response
        .dnd_hover_payload::<DraggedNote>()
        .filter(|note| note.vault == vault)
    else {
        ui.ctx().data_mut(|data| data.remove::<f64>(timer_id));
        return false;
    };
    let now = ui.input(|input| input.time);
    let since = ui
        .ctx()
        .data_mut(|data| *data.get_temp_mut_or_insert_with(timer_id, || now));
    ui.painter().rect_stroke(
        response.rect,
        3.0,
        egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
    );
    if let Some(note) = response.dnd_release_payload::<DraggedNote>() {
        *action = FileAction::MoveNote {
            source: note.path.clone(),
            folder: folder.to_path_buf(),
        };
    }
    ui.ctx().request_repaint_after(Duration::from_millis(100));
    now - since >= 0.6
}

// MoveFileW refuses an existing destination, including a file created after validation.
#[cfg(target_os = "windows")]
fn move_without_overwrite(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileW(source: *const u16, destination: *const u16) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Both arguments are live, NUL-terminated UTF-16 buffers.
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn move_without_overwrite(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)?;
    if let Err(error) = fs::remove_file(source) {
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    Ok(())
}

fn file_context_menu(ui: &mut egui::Ui, path: &Path, is_dir: bool, action: &mut FileAction) {
    let label = if is_dir {
        "Open folder in file explorer"
    } else {
        "Show in file explorer"
    };
    if ui.button(label).clicked() {
        *action = FileAction::ShowInFileManager(path.to_path_buf());
        ui.close_menu();
    }
    ui.separator();
    if is_dir {
        if ui.button("New folder here…").clicked() {
            *action = FileAction::CreateFolder(path.to_path_buf());
            ui.close_menu();
        }
        if ui.button("New note here").clicked() {
            *action = FileAction::CreateNote(path.to_path_buf());
            ui.close_menu();
        }
    } else if ui.button("Move to trash").clicked() {
        *action = FileAction::Delete(path.to_path_buf());
        ui.close_menu();
    }
}

fn file_manager_command(path: &Path) -> std::io::Result<std::process::Command> {
    // Avoid canonicalize: Windows extended-length paths are not understood by Explorer.
    let path = std::env::current_dir()?.join(path);
    let is_dir = fs::metadata(&path)?.is_dir();
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("explorer.exe");
        if !is_dir {
            command.arg("/select,");
        }
        command.arg(path);
        Ok(command)
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        if !is_dir {
            command.arg("-R");
        }
        command.arg(path);
        Ok(command)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(if is_dir {
            &path
        } else {
            path.parent().unwrap_or(&path)
        });
        Ok(command)
    }
}

pub struct NotesApp {
    search: String,
    status: String,
    saved_text: String,
    notes: Vec<(PathBuf, String)>,
    workspace_dir: PathBuf,
    current_file_path: Option<PathBuf>,
    editor_text: String,
    image_view: Option<(String, Arc<[u8]>)>,
    commonmark_cache: CommonMarkCache,
    file_tree: FileNode,
    is_creating_note: bool,
    new_note_name: String,
    display_mode: DisplayMode,
    scan: Option<mpsc::Receiver<ScanMessage>>,
    content_requested: bool,
    content_ready: bool,
    search_contents: bool,
    scan_cancel: Arc<AtomicBool>,
    expanded: HashSet<PathBuf>,
    backlinks: Vec<PathBuf>,
    search_results: Vec<PathBuf>,
    indexed_query: Option<String>,
    show_connections: bool,
    focus_editor: bool,
    last_edit: Option<Instant>,
    light_theme: bool,
    create_note_target_dir: Option<PathBuf>,
    image_cache: std::collections::HashMap<String, PathBuf>,
    create_folder_target: Option<PathBuf>,
    new_folder_name: String,
    folder_error: String,
}

struct FileNode {
    path: PathBuf,
    is_dir: bool,
    children: Option<Vec<FileNode>>,
}

fn visible_path(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    !name.starts_with('.') && !matches!(name.as_ref(), "target" | "node_modules" | "venv")
}

fn valid_note_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
}

fn wiki_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    for part in text.split("[[").skip(1) {
        if let Some((link, _)) = part.split_once("]]") {
            let target = link
                .split('|')
                .next()
                .unwrap_or("")
                .split('#')
                .next()
                .unwrap_or("")
                .trim();
            if !target.is_empty() && !links.iter().any(|s| s == target) {
                links.push(target.to_string());
            }
        }
    }
    links
}

fn matches_note(root: &Path, path: &Path, link: &str) -> bool {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    let target = link.strip_suffix(".md").unwrap_or(link);
    relative.eq_ignore_ascii_case(target)
        || path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .eq_ignore_ascii_case(target)
}

struct VaultIndex {
    content_loaded: bool,
    skipped: usize,
    notes: Vec<(PathBuf, String)>,
    images: std::collections::HashMap<String, PathBuf>,
}

enum ScanMessage {
    Preview(FileNode),
    Tree(FileNode),
    Ready(VaultIndex),
    Error(String),
}

fn scan_vault(
    root: PathBuf,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ScanMessage>,
    load_content: bool,
) {
    let mut shallow = empty_tree(root.clone());
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) => {
            let _ = tx.send(ScanMessage::Error(e.to_string()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(kind) = entry.file_type() {
            if visible_path(&path) && !kind.is_symlink() && (kind.is_dir() || kind.is_file()) {
                shallow.children.as_mut().unwrap().push(FileNode {
                    path,
                    is_dir: kind.is_dir(),
                    children: Some(Vec::new()),
                });
            }
        }
    }
    sort_nodes(shallow.children.as_mut().unwrap());
    if tx.send(ScanMessage::Preview(shallow)).is_err() {
        return;
    }
    let mut children: std::collections::HashMap<PathBuf, Vec<FileNode>> =
        std::collections::HashMap::new();
    let mut paths = Vec::new();
    let mut images = std::collections::HashMap::new();
    let mut skipped = 0;
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || visible_path(e.path()))
    {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if entry.depth() == 0 || entry.file_type().is_symlink() {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().is_dir();
        if is_dir || entry.file_type().is_file() {
            children
                .entry(path.parent().unwrap_or(&root).to_path_buf())
                .or_default()
                .push(FileNode {
                    path: path.to_path_buf(),
                    is_dir,
                    children: None,
                });
        }
        if entry.file_type().is_file() {
            if is_note(path) {
                paths.push(path.to_path_buf());
            } else if path.extension().is_some_and(|e| {
                matches!(
                    e.to_string_lossy().to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp"
                )
            }) {
                images.insert(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    path.to_path_buf(),
                );
            }
        }
    }
    fn assemble(
        path: PathBuf,
        all: &mut std::collections::HashMap<PathBuf, Vec<FileNode>>,
    ) -> FileNode {
        let mut nodes = all.remove(&path).unwrap_or_default();
        for node in &mut nodes {
            if node.is_dir {
                *node = assemble(node.path.clone(), all);
            }
        }
        sort_nodes(&mut nodes);
        FileNode {
            path,
            is_dir: true,
            children: Some(nodes),
        }
    }
    if tx
        .send(ScanMessage::Tree(assemble(root, &mut children)))
        .is_err()
    {
        return;
    }
    let mut notes = Vec::new();
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if !load_content {
            notes.push((path, String::new()));
            continue;
        }
        if !fs::metadata(&path).is_ok_and(|m| m.len() <= 2 * 1024 * 1024) {
            skipped += 1;
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(text) => notes.push((path, text)),
            Err(_) => skipped += 1,
        }
    }
    if !cancel.load(Ordering::Relaxed) {
        let _ = tx.send(ScanMessage::Ready(VaultIndex {
            content_loaded: load_content,
            notes,
            images,
            skipped,
        }));
    }
}

fn sort_nodes(nodes: &mut [FileNode]) {
    nodes.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
}

#[derive(Debug, PartialEq)]
enum OpenKind {
    Markdown,
    Text,
    Image,
    External,
}

fn open_kind(path: &Path) -> OpenKind {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => OpenKind::Markdown,
        "txt" | "json" | "jsonc" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "log"
        | "csv" | "tsv" | "xml" | "html" | "css" | "js" | "ts" | "py" | "rs" | "c" | "cpp"
        | "h" | "sh" | "ps1" | "sql" => OpenKind::Text,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "svg" => {
            OpenKind::Image
        }
        _ => OpenKind::External,
    }
}

fn text_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    path: &Path,
    numbered: bool,
) -> egui::text_edit::TextEditOutput {
    ui.horizontal_top(|ui| {
        let gutter_left = ui.cursor().left();
        let gutter_width = if numbered {
            (text.bytes().filter(|b| *b == b'\n').count() + 1)
                .to_string()
                .len() as f32
                * 10.0
                + 20.0
        } else {
            0.0
        };
        if numbered {
            ui.add_space(gutter_width);
        }
        let font = if numbered {
            egui::FontId::monospace(16.0)
        } else {
            egui::FontId::proportional(18.0)
        };
        let output = egui::TextEdit::multiline(text)
            .id_source(path)
            .font(font.clone())
            .desired_width(ui.available_width())
            .desired_rows(30)
            .frame(false)
            .show(ui);
        if numbered {
            let mut line = 1;
            let mut start = true;
            let painter = ui.painter().with_clip_rect(ui.clip_rect());
            for row in &output.galley.rows {
                if start {
                    let y = output.galley_pos.y + row.rect.top();
                    if y + row.rect.height() >= ui.clip_rect().top() && y <= ui.clip_rect().bottom()
                    {
                        painter.text(
                            egui::pos2(gutter_left + gutter_width - 8.0, y),
                            egui::Align2::RIGHT_TOP,
                            line.to_string(),
                            font.clone(),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
                start = row.ends_with_newline;
                if start {
                    line += 1;
                }
            }
        }
        output
    })
    .inner
}

fn is_note(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("txt"))
}

fn empty_tree(path: PathBuf) -> FileNode {
    FileNode {
        path,
        is_dir: true,
        children: Some(Vec::new()),
    }
}

fn tree_rows<'a>(node: &'a FileNode, expanded: &HashSet<PathBuf>, rows: &mut Vec<&'a FileNode>) {
    if let Some(children) = &node.children {
        for child in children {
            rows.push(child);
            if child.is_dir && expanded.contains(&child.path) {
                tree_rows(child, expanded, rows);
            }
        }
    }
}

fn apply_theme(ctx: &egui::Context, light: bool) {
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    if !light {
        visuals.panel_fill = egui::Color32::from_rgb(27, 28, 32);
        visuals.window_fill = egui::Color32::from_rgb(32, 33, 38);
        visuals.extreme_bg_color = egui::Color32::from_rgb(23, 24, 28);
        visuals.override_text_color = Some(egui::Color32::from_rgb(220, 222, 227));
        visuals.selection.bg_fill = egui::Color32::from_rgb(58, 69, 97);
        visuals.hyperlink_color = egui::Color32::from_rgb(166, 187, 238);
    }
    ctx.set_visuals(visuals);
}

impl NotesApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut workspace_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut display_mode = DisplayMode::EditOnly;

        if let Some(storage) = cc.storage {
            if let Some(saved_dir) = eframe::get_value::<PathBuf>(storage, "workspace_dir") {
                if saved_dir.exists() {
                    workspace_dir = saved_dir;
                }
            }
            if eframe::get_value::<u32>(storage, "layout_version") == Some(2) {
                if let Some(saved_mode) = eframe::get_value::<DisplayMode>(storage, "display_mode")
                {
                    display_mode = saved_mode;
                }
            }
        }

        let file_tree = empty_tree(workspace_dir.clone());
        let image_cache = std::collections::HashMap::new();
        let light_theme = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "light_theme"))
            .unwrap_or(false);
        apply_theme(&cc.egui_ctx, light_theme);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(8.0, 6.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        cc.egui_ctx.set_style(style);
        let notes = Vec::new();
        let mut app = Self {
            search: String::new(),
            status: "Stored on your device".into(),
            saved_text: String::new(),
            notes,
            workspace_dir,
            current_file_path: None,
            editor_text: String::new(),
            image_view: None,
            commonmark_cache: CommonMarkCache::default(),
            file_tree,
            is_creating_note: false,
            new_note_name: String::new(),
            display_mode,
            scan: None,
            content_requested: false,
            content_ready: false,
            search_contents: false,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            expanded: HashSet::new(),
            backlinks: Vec::new(),
            search_results: Vec::new(),
            indexed_query: None,
            show_connections: false,
            focus_editor: false,
            last_edit: None,
            light_theme,
            create_note_target_dir: None,
            image_cache,
            create_folder_target: None,
            new_folder_name: String::new(),
            folder_error: String::new(),
        };
        if let Some(storage) = cc.storage {
            if let Some(path) =
                eframe::get_value::<Option<PathBuf>>(storage, "current_file").flatten()
            {
                if path.starts_with(&app.workspace_dir) && path.is_file() {
                    app.open_file(&path);
                }
            }
        }
        app.refresh();
        app
    }

    fn open_file(&mut self, path: &Path) {
        if !self.save_current_file() {
            return;
        }
        match open_kind(path) {
            OpenKind::External => {
                if let Err(e) = open::that(path) {
                    self.status = format!("Could not open with the default app: {e}");
                }
                return;
            }
            OpenKind::Image => {
                let result = if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
                {
                    fs::read(path)
                        .map(|bytes| ("svg", bytes))
                        .map_err(|e| e.to_string())
                } else {
                    image::open(path)
                        .and_then(|image| {
                            let mut buffer = std::io::Cursor::new(Vec::new());
                            image.write_to(&mut buffer, image::ImageFormat::Png)?;
                            Ok(("png", buffer.into_inner()))
                        })
                        .map_err(|e| e.to_string())
                };
                match result {
                    Ok((ext, bytes)) => {
                        self.image_view = Some((
                            format!(
                                "bytes://selva-{}.{}",
                                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                                ext
                            ),
                            bytes.into(),
                        ));
                        self.current_file_path = Some(path.to_path_buf());
                        self.editor_text.clear();
                        self.saved_text.clear();
                        self.last_edit = None;
                        self.status = "Image preview".into();
                    }
                    Err(e) => self.status = format!("Could not load image: {e}"),
                }
                return;
            }
            _ => {}
        }
        match fs::read_to_string(path) {
            Ok(content) => {
                self.image_view = None;
                self.current_file_path = Some(path.to_path_buf());
                self.saved_text = content.clone();
                self.editor_text = content;
                self.focus_editor = true;
                self.last_edit = None;
                self.refresh_backlinks();
                self.status = "Saved".into();
            }
            Err(e) => self.status = format!("Could not read file: {e}"),
        }
    }

    fn save_current_file(&mut self) -> bool {
        if self.editor_text == self.saved_text {
            return true;
        }
        if let Some(path) = &self.current_file_path {
            match fs::read_to_string(path) {
                Ok(disk) if disk == self.saved_text => {}
                _ => {
                    self.status = "File changed on disk. Copy your draft before reloading.".into();
                    return false;
                }
            }
            match fs::write(path, &self.editor_text) {
                Ok(()) => {
                    self.saved_text = self.editor_text.clone();
                    self.status = "Saved".into();
                    self.last_edit = None;
                    self.indexed_query = None;
                    if let Some(note) = self.notes.iter_mut().find(|n| &n.0 == path) {
                        note.1 = self.editor_text.clone();
                    }
                }
                Err(e) => {
                    self.status = format!("Could not save: {e}");
                    return false;
                }
            }
        }
        true
    }
    fn refresh(&mut self) {
        self.scan_cancel.store(true, Ordering::Relaxed);
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.scan_cancel.clone();
        let root = self.workspace_dir.clone();
        let (tx, rx) = mpsc::channel();
        self.scan = Some(rx);
        let load_content = self.content_requested;
        self.content_ready = false;
        self.status = if load_content {
            "Indexing contents…"
        } else {
            "Opening vault…"
        }
        .into();
        std::thread::spawn(move || scan_vault(root, cancel, tx, load_content));
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        // Keep targets under the pointer stable until the drag has finished.
        if egui::DragAndDrop::has_payload_of_type::<DraggedNote>(ctx) {
            return;
        }
        if let Some(rx) = &self.scan {
            match rx.try_recv() {
                Ok(ScanMessage::Preview(tree)) => {
                    // A shallow startup preview must not collapse an already populated tree.
                    if self
                        .file_tree
                        .children
                        .as_ref()
                        .map_or(true, |children| children.is_empty())
                    {
                        self.file_tree = tree;
                    }
                    ctx.request_repaint();
                }
                Ok(ScanMessage::Tree(tree)) => {
                    self.file_tree = tree;
                    ctx.request_repaint();
                }
                Ok(ScanMessage::Ready(index)) => {
                    self.content_ready = index.content_loaded;
                    self.notes = index.notes;
                    if let Some(current) = &self.current_file_path {
                        if let Some(note) = self.notes.iter_mut().find(|(p, _)| p == current) {
                            note.1 = self.editor_text.clone();
                        }
                    }
                    self.image_cache = index.images;
                    self.scan = None;
                    self.indexed_query = None;
                    self.refresh_backlinks();
                    self.status = format!(
                        "{} notes · {}",
                        self.notes.len(),
                        if self.content_ready {
                            "Contents indexed"
                        } else {
                            "Vault ready"
                        }
                    );
                    if index.skipped > 0 {
                        self.status.push_str(&format!(
                            " · {} files skipped (unreadable or over 2 MB)",
                            index.skipped
                        ));
                    }
                }
                Ok(ScanMessage::Error(error)) => {
                    self.scan = None;
                    self.content_requested = false;
                    self.status = format!("Could not open vault: {error}");
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan = None;
                    self.content_requested = false;
                    self.status = "Loading interrupted. Click Refresh to retry.".into();
                }
            }
        }
        if self.content_requested && !self.content_ready && self.scan.is_none() {
            self.refresh();
        }
    }

    fn refresh_backlinks(&mut self) {
        self.backlinks.clear();
        if let Some(current) = &self.current_file_path {
            for (path, text) in &self.notes {
                if path != current
                    && wiki_links(text)
                        .iter()
                        .any(|link| matches_note(&self.workspace_dir, current, link))
                {
                    self.backlinks.push(path.clone());
                }
            }
        }
    }

    fn choose_vault(&mut self) {
        if !self.save_current_file() {
            return;
        }
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Open your vault")
            .pick_folder()
        {
            self.content_requested = false;
            self.content_ready = false;
            self.search_contents = false;
            self.show_connections = false;
            self.workspace_dir = folder;
            self.create_folder_target = None;
            self.is_creating_note = false;
            self.current_file_path = None;
            self.image_view = None;
            self.editor_text.clear();
            self.saved_text.clear();
            self.notes.clear();
            self.image_cache.clear();
            self.backlinks.clear();
            self.expanded.clear();
            self.search.clear();
            self.search_results.clear();
            self.indexed_query = None;
            self.file_tree = empty_tree(self.workspace_dir.clone());
            self.refresh();
        }
    }

    fn move_note(&mut self, source: &Path, folder: &Path) {
        let result = (|| -> Result<Option<PathBuf>, String> {
            let root = self
                .workspace_dir
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let actual_source = source.canonicalize().map_err(|e| e.to_string())?;
            let actual_folder = folder.canonicalize().map_err(|e| e.to_string())?;
            if !actual_source.starts_with(&root)
                || !actual_folder.starts_with(&root)
                || !fs::symlink_metadata(source)
                    .map_err(|e| e.to_string())?
                    .is_file()
                || !actual_folder.is_dir()
                || !matches!(open_kind(source), OpenKind::Markdown | OpenKind::Text)
            {
                return Err("Choose a note and a folder inside this vault.".into());
            }
            if actual_source.parent() == Some(actual_folder.as_path()) {
                return Ok(None);
            }
            let destination = folder.join(source.file_name().ok_or("Invalid note name")?);
            if destination.try_exists().map_err(|e| e.to_string())? {
                return Err(
                    "A file with this name already exists in the destination folder.".into(),
                );
            }
            Ok(Some(destination))
        })();
        let destination = match result {
            Ok(Some(path)) => path,
            Ok(None) => {
                self.status = "The note is already in this folder.".into();
                return;
            }
            Err(error) => {
                self.status = format!("Could not move note: {error}");
                return;
            }
        };
        let is_open = self.current_file_path.as_deref() == Some(source);
        if is_open && !self.save_current_file() {
            return;
        }
        if let Err(error) = move_without_overwrite(source, &destination) {
            self.status = format!("Could not move note: {error}");
            return;
        }
        if is_open {
            self.current_file_path = Some(destination.clone());
            self.last_edit = None;
        }
        for (path, _) in &mut self.notes {
            if path == source {
                *path = destination.clone();
            }
        }
        for ancestor in folder.ancestors() {
            if ancestor.starts_with(&self.workspace_dir) {
                self.expanded.insert(ancestor.to_path_buf());
            }
        }
        self.search.clear();
        self.indexed_query = None;
        self.refresh_backlinks();
        self.refresh();
        self.status = format!("Moved to {}", destination.display());
    }

    fn create_folder(&mut self) {
        let Some(parent) = self.create_folder_target.as_ref() else {
            return;
        };
        let name = self.new_folder_name.trim();
        if !valid_note_name(name) || !visible_path(Path::new(name)) {
            self.folder_error =
                "Use a visible folder name without a path or reserved characters.".into();
            return;
        }
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => {
                for ancestor in path.ancestors().skip(1) {
                    if ancestor.starts_with(&self.workspace_dir) {
                        self.expanded.insert(ancestor.to_path_buf());
                    }
                }
                self.search.clear();
                self.create_folder_target = None;
                self.folder_error.clear();
                self.refresh();
            }
            Err(error) => self.folder_error = format!("Could not create folder: {error}"),
        }
    }

    fn create_note(&mut self) {
        let name = self.new_note_name.trim();
        if !valid_note_name(name) {
            self.status = "Invalid name: use a title without a path.".into();
            return;
        }
        let name = if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        };
        let path = self
            .create_note_target_dir
            .as_ref()
            .unwrap_or(&self.workspace_dir)
            .join(name);
        if !self.save_current_file() {
            return;
        }
        use std::io::Write;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let title = path.file_stem().unwrap_or_default().to_string_lossy();
                if let Err(e) = write!(file, "# {title}\n\n") {
                    self.status = e.to_string();
                    return;
                }
                self.refresh();
                self.open_file(&path);
                self.is_creating_note = false;
            }
            Err(e) => self.status = format!("Could not create note: {e}"),
        }
    }
}

impl Drop for NotesApp {
    fn drop(&mut self) {
        self.scan_cancel.store(true, Ordering::Relaxed);
    }
}

impl eframe::App for NotesApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "layout_version", &2_u32);
        eframe::set_value(storage, "light_theme", &self.light_theme);
        eframe::set_value(storage, "current_file", &self.current_file_path);
        eframe::set_value(storage, "workspace_dir", &self.workspace_dir);
        eframe::set_value(storage, "display_mode", &self.display_mode);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl NotesApp {
    fn ui(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            egui::DragAndDrop::clear_payload(ctx);
        }
        if let Some(note) = egui::DragAndDrop::payload::<DraggedNote>(ctx) {
            if note.vault != self.workspace_dir {
                egui::DragAndDrop::clear_payload(ctx);
            }
        }
        self.poll_scan(ctx);
        if self
            .last_edit
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(650))
        {
            self.last_edit = None;
            self.save_current_file();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.save_current_file();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::N)) {
            self.is_creating_note = true;
            self.new_note_name.clear();
            self.create_note_target_dir = None;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.save_current_file() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.small(&self.status);
                ui.separator();
                ui.small(format!(
                    "{} words · {} characters",
                    self.editor_text.split_whitespace().count(),
                    self.editor_text.chars().count()
                ));
                if self.editor_text != self.saved_text {
                    ui.colored_label(egui::Color32::LIGHT_RED, "Unsaved changes");
                }
            });
        });
        let mut action = FileAction::None;

        egui::SidePanel::left("vault_sidebar_v2")
            .resizable(true)
            .default_width(250.0)
            .width_range(200.0..=360.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 8.0);
                    ui.spacing_mut().button_padding = egui::vec2(8.0, 6.0);
                    // A compact vault header; secondary actions stay in its menu.
                    ui.horizontal(|ui| {
                        let name = self
                            .workspace_dir
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let short = if name.chars().count() > 18 {
                            format!("{}…", name.chars().take(17).collect::<String>())
                        } else {
                            name.clone()
                        };
                        let width = (ui.available_width() - 38.0).max(100.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 30.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.menu_button(
                                    egui::RichText::new(short).size(17.0).strong(),
                                    |ui| {
                                        ui.set_min_width(190.0);
                                        ui.weak("VAULT");
                                        if ui.button("Open folder in file explorer").clicked() {
                                            action = FileAction::ShowInFileManager(
                                                self.workspace_dir.clone(),
                                            );
                                            ui.close_menu();
                                        }
                                        if ui.button("New folder…").clicked() {
                                            action = FileAction::CreateFolder(
                                                self.workspace_dir.clone(),
                                            );
                                            ui.close_menu();
                                        }
                                        if ui.button("Open another vault…").clicked() {
                                            self.choose_vault();
                                            ui.close_menu();
                                        }
                                        if ui.button("Refresh files").clicked() {
                                            self.refresh();
                                            ui.close_menu();
                                        }
                                        if ui.button("Collapse folders").clicked() {
                                            self.expanded.clear();
                                            ui.close_menu();
                                        }
                                        if ui.button("Open daily note").clicked() {
                                            let name =
                                                chrono::Local::now().format("%Y-%m-%d").to_string();
                                            let path =
                                                self.workspace_dir.join(format!("{name}.md"));
                                            if path.exists() {
                                                self.open_file(&path);
                                            } else {
                                                self.new_note_name = name;
                                                self.create_note_target_dir = None;
                                                self.create_note();
                                            }
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui
                                            .checkbox(
                                                &mut self.show_connections,
                                                "Show links panel",
                                            )
                                            .changed()
                                            && self.show_connections
                                        {
                                            self.content_requested = true;
                                        }
                                        if ui
                                            .checkbox(&mut self.light_theme, "Light theme")
                                            .changed()
                                        {
                                            apply_theme(ctx, self.light_theme);
                                        }
                                    },
                                )
                                .response
                                .on_hover_text(format!(
                                    "{}\nVault actions",
                                    self.workspace_dir.display()
                                ));
                            },
                        );
                        if ui
                            .add_sized(
                                [30.0, 30.0],
                                egui::Button::new(egui::RichText::new("+").size(21.0)).frame(false),
                            )
                            .on_hover_text("New note · Ctrl+N")
                            .clicked()
                        {
                            self.is_creating_note = true;
                            self.new_note_name.clear();
                            self.create_note_target_dir = None;
                        }
                    });
                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .rounding(6.0)
                        .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let width = (ui.available_width() - 28.0).max(60.0);
                                let search = ui.add(
                                    egui::TextEdit::singleline(&mut self.search)
                                        .frame(false)
                                        .desired_width(width)
                                        .hint_text(if self.search_contents {
                                            "Search contents…"
                                        } else {
                                            "Search notes…"
                                        }),
                                );
                                if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::K))
                                {
                                    search.request_focus();
                                }
                                ui.menu_button("…", |ui| {
                                    ui.weak("SEARCH IN");
                                    if ui
                                        .selectable_label(!self.search_contents, "File names")
                                        .clicked()
                                    {
                                        self.search_contents = false;
                                        self.indexed_query = None;
                                        ui.close_menu();
                                    }
                                    if ui
                                        .selectable_label(
                                            self.search_contents,
                                            "Names and contents",
                                        )
                                        .clicked()
                                    {
                                        self.search_contents = true;
                                        self.content_requested = true;
                                        self.indexed_query = None;
                                        ui.close_menu();
                                    }
                                })
                                .response
                                .on_hover_text("Search options");
                            });
                        });
                    egui::Frame::none()
                        .fill(ui.visuals().faint_bg_color)
                        .rounding(6.0)
                        .inner_margin(3.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.horizontal(|ui| {
                                let wide = ctx.screen_rect().width() >= 1050.0;
                                let count = if wide { 3.0 } else { 2.0 };
                                let width = (ui.available_width() - (count - 1.0) * 2.0) / count;
                                for (mode, label) in [
                                    (DisplayMode::EditOnly, "Write"),
                                    (DisplayMode::ViewOnly, "Read"),
                                    (DisplayMode::EditAndPreview, "Split"),
                                ] {
                                    if label == "Split" && !wide {
                                        continue;
                                    }
                                    let selected = self.display_mode == mode;
                                    if ui
                                        .add_sized(
                                            [width, 26.0],
                                            egui::SelectableLabel::new(selected, label),
                                        )
                                        .clicked()
                                    {
                                        self.display_mode = mode;
                                    }
                                }
                            });
                        });
                    ui.add_space(3.0);
                });
                ui.separator();
                let root_response = explorer_row(ui, &self.workspace_dir, "Vault root", false)
                    .on_hover_text("Drop a note here to move it to the vault root");
                note_drop_target(
                    ui,
                    &root_response,
                    &self.workspace_dir,
                    &self.workspace_dir,
                    &mut action,
                );
                if self.search.trim().is_empty()
                    || egui::DragAndDrop::has_payload_of_type::<DraggedNote>(ctx)
                {
                    let mut rows = Vec::new();
                    tree_rows(&self.file_tree, &self.expanded, &mut rows);
                    egui::ScrollArea::vertical()
                        .id_source("vault_files")
                        .auto_shrink([false, false])
                        .show_rows(ui, EXPLORER_ROW_HEIGHT, rows.len(), |ui, range| {
                            for index in range {
                                let node = rows[index];
                                let depth = node
                                    .path
                                    .strip_prefix(&self.workspace_dir)
                                    .unwrap_or(&node.path)
                                    .components()
                                    .count()
                                    .saturating_sub(1);
                                ui.horizontal(|ui| {
                                    ui.add_space((depth as f32 * 14.0).min(70.0));
                                    let name =
                                        node.path.file_name().unwrap_or_default().to_string_lossy();
                                    let (icon_rect, icon_response) = ui.allocate_exact_size(
                                        egui::vec2(12.0, 20.0),
                                        egui::Sense::click(),
                                    );
                                    if node.is_dir {
                                        let center = icon_rect.center();
                                        let points = if self.expanded.contains(&node.path) {
                                            vec![
                                                center + egui::vec2(-4.0, -2.0),
                                                center + egui::vec2(0.0, 2.0),
                                                center + egui::vec2(4.0, -2.0),
                                            ]
                                        } else {
                                            vec![
                                                center + egui::vec2(-2.0, -4.0),
                                                center + egui::vec2(2.0, 0.0),
                                                center + egui::vec2(-2.0, 4.0),
                                            ]
                                        };
                                        ui.painter().add(egui::Shape::line(
                                            points,
                                            egui::Stroke::new(1.5_f32, ui.visuals().text_color()),
                                        ));
                                    }
                                    let label = name.into_owned();
                                    let response = explorer_row(
                                        ui,
                                        &node.path,
                                        &label,
                                        self.current_file_path.as_ref() == Some(&node.path),
                                    )
                                    .on_hover_text(node.path.display().to_string());
                                    let response = if node.is_dir {
                                        if note_drop_target(
                                            ui,
                                            &response.union(icon_response.clone()),
                                            &node.path,
                                            &self.workspace_dir,
                                            &mut action,
                                        ) {
                                            self.expanded.insert(node.path.clone());
                                        }
                                        response
                                    } else {
                                        note_drag_source(response, &node.path, &self.workspace_dir)
                                    };
                                    if response.clicked()
                                        || (node.is_dir && icon_response.clicked())
                                    {
                                        if node.is_dir {
                                            if !self.expanded.remove(&node.path) {
                                                self.expanded.insert(node.path.clone());
                                            }
                                        } else {
                                            action = FileAction::Open(node.path.clone());
                                        }
                                    }
                                    response.context_menu(|ui| {
                                        file_context_menu(ui, &node.path, node.is_dir, &mut action);
                                    });
                                });
                            }
                        });
                    if rows.is_empty() && self.scan.is_none() {
                        ui.label("No files in this folder.");
                    }
                } else {
                    let query = self.search.trim().to_lowercase();
                    if self.indexed_query.as_ref() != Some(&query) {
                        self.search_results = self
                            .notes
                            .iter()
                            .filter(|(path, text)| {
                                path.strip_prefix(&self.workspace_dir)
                                    .unwrap_or(path)
                                    .to_string_lossy()
                                    .to_lowercase()
                                    .contains(&query)
                                    || (self.search_contents
                                        && text.to_lowercase().contains(&query))
                            })
                            .map(|(p, _)| p.clone())
                            .collect();
                        self.indexed_query = Some(query);
                    }
                    ui.small(format!("{} results", self.search_results.len()));
                    egui::ScrollArea::vertical()
                        .id_source("search_results")
                        .show_rows(
                            ui,
                            EXPLORER_ROW_HEIGHT,
                            self.search_results.len(),
                            |ui, range| {
                                for index in range {
                                    let path = &self.search_results[index];
                                    let response = explorer_row(
                                        ui,
                                        path,
                                        &path
                                            .strip_prefix(&self.workspace_dir)
                                            .unwrap_or(path)
                                            .display()
                                            .to_string(),
                                        self.current_file_path.as_ref() == Some(path),
                                    );
                                    let response =
                                        note_drag_source(response, path, &self.workspace_dir);
                                    if response.clicked() {
                                        action = FileAction::Open(path.clone());
                                    }
                                    response.context_menu(|ui| {
                                        file_context_menu(ui, path, false, &mut action);
                                    });
                                }
                            },
                        );
                }
            });
        if let Some(parent) = self.create_folder_target.clone() {
            egui::Window::new("New folder")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Inside: {}", parent.display()));
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut self.new_folder_name)
                            .hint_text("Folder name")
                            .desired_width(320.0),
                    );
                    if self.new_folder_name.is_empty() && !name.has_focus() {
                        name.request_focus();
                    }
                    if !self.folder_error.is_empty() {
                        ui.colored_label(ui.visuals().error_fg_color, &self.folder_error);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Create folder").clicked()
                            || (name.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            self.create_folder();
                        }
                        if ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            self.create_folder_target = None;
                        }
                    });
                });
        }
        if self.is_creating_note {
            egui::Window::new("New note")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Note title");
                    let title = ui.add(
                        egui::TextEdit::singleline(&mut self.new_note_name)
                            .hint_text("An idea to remember")
                            .desired_width(320.0),
                    );
                    if !title.has_focus() && self.new_note_name.is_empty() {
                        title.request_focus();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Create note").clicked()
                            || (title.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            self.create_note();
                            self.display_mode = DisplayMode::EditOnly;
                        }
                        if ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            self.is_creating_note = false;
                        }
                    });
                });
        }

        match action {
            FileAction::MoveNote { source, folder } => self.move_note(&source, &folder),
            FileAction::ShowInFileManager(path) => {
                if let Err(error) =
                    file_manager_command(&path).and_then(|mut command| command.spawn())
                {
                    self.status = format!("Could not open file explorer: {error}");
                }
            }
            FileAction::Open(path) => self.open_file(&path),
            FileAction::Delete(path) => {
                if !self.save_current_file() {
                    return;
                }
                let trash = self.workspace_dir.join(".selva-trash");
                let destination = trash.join(format!(
                    "{}-{}",
                    chrono::Local::now()
                        .timestamp_nanos_opt()
                        .unwrap_or_default(),
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                match fs::create_dir_all(&trash).and_then(|_| fs::rename(&path, destination)) {
                    Ok(()) => {
                        self.status = "Note moved to .selva-trash (recoverable).".into();
                        if self.current_file_path.as_ref() == Some(&path) {
                            self.current_file_path = None;
                            self.image_view = None;
                            self.editor_text.clear();
                            self.saved_text.clear();
                        }
                        self.refresh();
                    }
                    Err(e) => self.status = format!("Could not move to trash: {e}"),
                }
            }
            FileAction::CreateNote(path) => {
                self.create_folder_target = None;
                self.is_creating_note = true;
                self.new_note_name = String::from("New note.md");
                self.create_note_target_dir = Some(path);
            }
            FileAction::CreateFolder(path) => {
                self.is_creating_note = false;
                self.create_folder_target = Some(path);
                self.new_folder_name.clear();
                self.folder_error.clear();
            }
            FileAction::None => {}
        }

        let is_markdown = self
            .current_file_path
            .as_ref()
            .is_some_and(|p| open_kind(p) == OpenKind::Markdown);
        // Preprocess obsidian-style wiki-links
        let mut processed_text = self.editor_text.clone();
        if is_markdown {
            let re = regex::Regex::new(r"!\[\[(.*?)\]\]").unwrap();
            processed_text = re
                .replace_all(&processed_text, |caps: &regex::Captures| {
                    let img_name = &caps[1];
                    let filename = img_name.split('|').next().unwrap_or(img_name);
                    if let Some(path) = self.image_cache.get(filename) {
                        format!(
                            "![{}](<{}://{}>)",
                            filename,
                            "file",
                            path.to_string_lossy().replace('\\', "/")
                        )
                    } else {
                        format!("![[{}]]", img_name)
                    }
                })
                .to_string();
        }

        if self.show_connections && is_markdown && ctx.screen_rect().width() >= 1050.0 {
            let mut navigate = None;
            egui::SidePanel::right("connections_v2")
                .default_width(220.0)
                .width_range(180.0..=300.0)
                .show(ctx, |ui| {
                    ui.add_space(10.0);
                    ui.heading("Connections");
                    ui.small("Connect ideas with [[Note name]]");
                    ui.separator();
                    ui.label("OUTGOING LINKS");
                    let links = wiki_links(&self.editor_text);
                    if links.is_empty() {
                        ui.small("No links yet.");
                    }
                    for link in links {
                        let candidates: Vec<_> = self
                            .notes
                            .iter()
                            .filter(|(p, _)| matches_note(&self.workspace_dir, p, &link))
                            .collect();
                        if candidates.is_empty() {
                            ui.weak(format!("{link} · not created"));
                        }
                        for (path, _) in candidates {
                            if ui
                                .link(
                                    path.strip_prefix(&self.workspace_dir)
                                        .unwrap_or(path)
                                        .display()
                                        .to_string(),
                                )
                                .clicked()
                            {
                                navigate = Some(path.clone());
                            }
                        }
                    }
                    ui.add_space(16.0);
                    ui.label("BACKLINKS");
                    for path in &self.backlinks {
                        if ui
                            .link(
                                path.strip_prefix(&self.workspace_dir)
                                    .unwrap_or(path)
                                    .display()
                                    .to_string(),
                            )
                            .clicked()
                        {
                            navigate = Some(path.clone());
                        }
                    }
                    if !self.content_ready {
                        ui.small("Indexing backlinks…");
                    } else if self.backlinks.is_empty() {
                        ui.small("No notes link to this page.");
                    }
                });
            if let Some(path) = navigate {
                self.open_file(&path);
                ctx.request_repaint();
                return;
            }
        }

        // Right panel for preview, ONLY in split mode
        let show_right_preview = is_markdown
            && self.display_mode == DisplayMode::EditAndPreview
            && ctx.screen_rect().width() >= 1050.0;

        if show_right_preview {
            egui::SidePanel::right("preview_panel_v2")
                .resizable(true)
                .default_width(360.0)
                .width_range(250.0..=500.0)
                .show(ctx, |ui| {
                    ui.heading("Preview");
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new("viewer").show(
                            ui,
                            &mut self.commonmark_cache,
                            &processed_text,
                        );
                    });
                });
        }

        // Central panel
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(path) = &self.current_file_path {
                let name = path.file_name().unwrap_or_default().to_string_lossy();

                ui.heading(name);
                ui.separator();

                if let Some((uri, bytes)) = &self.image_view {
                    egui::ScrollArea::both()
                        .id_source("image_view")
                        .show(ui, |ui| {
                            ui.add(
                                egui::Image::from_bytes(uri.clone(), bytes.clone())
                                    .max_width(ui.available_width())
                                    .shrink_to_fit(),
                            );
                        });
                } else if is_markdown && self.display_mode == DisplayMode::ViewOnly {
                    // Full screen preview
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new("central_viewer").show(
                            ui,
                            &mut self.commonmark_cache,
                            &processed_text,
                        );
                    });
                } else {
                    // Editor

                    let output = egui::ScrollArea::vertical()
                        .id_source(("editor_scroll", path))
                        .show(ui, |ui| {
                            text_editor(ui, &mut self.editor_text, path, !is_markdown)
                        })
                        .inner;

                    if self.focus_editor
                        && !self.is_creating_note
                        && self.create_folder_target.is_none()
                    {
                        output.response.request_focus();
                        self.focus_editor = false;
                    }
                    let mut pasted_image = false;
                    let is_ctrl_v =
                        ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::V));
                    if is_markdown && output.response.has_focus() && is_ctrl_v {
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            if let Ok(img_data) = clipboard.get_image() {
                                let target_dir = self.workspace_dir.join("_assets");
                                let _ = fs::create_dir_all(&target_dir);
                                let filename = format!(
                                    "Pasted image {}.png",
                                    chrono::Local::now().format("%Y%m%d%H%M%S")
                                );
                                let file_path = target_dir.join(&filename);
                                if let Some(img_buffer) = image::RgbaImage::from_raw(
                                    img_data.width.try_into().unwrap(),
                                    img_data.height.try_into().unwrap(),
                                    img_data.bytes.into_owned(),
                                ) {
                                    if img_buffer.save(&file_path).is_ok() {
                                        let insert_text = format!("![[{}]]", filename);
                                        if let Some(cursor_range) = output.cursor_range {
                                            let min = cursor_range
                                                .primary
                                                .ccursor
                                                .index
                                                .min(cursor_range.secondary.ccursor.index);
                                            let max = cursor_range
                                                .primary
                                                .ccursor
                                                .index
                                                .max(cursor_range.secondary.ccursor.index);
                                            let byte_min = self
                                                .editor_text
                                                .char_indices()
                                                .nth(min)
                                                .map(|(i, _)| i)
                                                .unwrap_or(self.editor_text.len());
                                            let byte_max = self
                                                .editor_text
                                                .char_indices()
                                                .nth(max)
                                                .map(|(i, _)| i)
                                                .unwrap_or(self.editor_text.len());
                                            self.editor_text
                                                .replace_range(byte_min..byte_max, &insert_text);
                                        } else {
                                            self.editor_text
                                                .push_str(&format!("\n{}\n", insert_text));
                                        }
                                        self.image_cache.insert(filename, file_path);
                                        pasted_image = true;
                                    }
                                }
                            }
                        }
                    }

                    if output.response.changed() || pasted_image {
                        self.last_edit = Some(Instant::now());
                        ctx.request_repaint_after(Duration::from_millis(700));
                    }
                }
            } else {
                ui.add_space(70.0);
                ui.vertical_centered(|ui| {
                    ui.heading("Your notes, front and center.");
                    ui.add_space(12.0);
                    ui.label("Open a note from the sidebar or start writing.");
                    ui.add_space(20.0);
                    if ui.button("+ Create a note").clicked() {
                        self.is_creating_note = true;
                        self.new_note_name.clear();
                        self.create_note_target_dir = None;
                    }
                    if ui.button("Open another vault").clicked() {
                        self.choose_vault();
                    }
                    ui.add_space(20.0);
                    ui.small("Ctrl+N  New note     Ctrl+K  Search     Ctrl+S  Save");
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_paths_and_invalid_names() {
        for name in ["", "..", "../note", "a/b", "a\\b", "C:note", "bad?", "end."] {
            assert!(!valid_note_name(name), "{name}");
        }
        assert!(valid_note_name("Le mie idee.md"));
    }
    #[test]
    fn extracts_aliases_headings_and_deduplicates() {
        assert_eq!(
            wiki_links("[[Idea|titolo]] [[Idea#Parte]] [[Altra]]"),
            vec!["Idea", "Altra"]
        );
    }
    #[test]
    fn resolves_nested_notes() {
        let root = Path::new("vault");
        let note = root.join("folder").join("Idea.md");
        assert!(matches_note(root, &note, "folder/Idea"));
        assert!(matches_note(root, &note, "Idea.md"));
        assert!(!matches_note(root, &note, "Other"));
    }
    #[test]
    fn excludes_generated_and_hidden_folders() {
        assert!(!visible_path(Path::new("target")));
        assert!(!visible_path(Path::new(".selva-trash")));
        assert!(visible_path(Path::new("Idee")));
    }
    fn fixture() -> PathBuf {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("qa-vaults")
            .join(format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_app(root: PathBuf) -> NotesApp {
        NotesApp {
            search: String::new(),
            status: "Ready".into(),
            saved_text: String::new(),
            notes: Vec::new(),
            workspace_dir: root.clone(),
            current_file_path: None,
            editor_text: String::new(),
            image_view: None,
            commonmark_cache: CommonMarkCache::default(),
            file_tree: empty_tree(root),
            is_creating_note: false,
            new_note_name: String::new(),
            display_mode: DisplayMode::EditOnly,
            scan: None,
            content_requested: false,
            content_ready: false,
            search_contents: false,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            expanded: HashSet::new(),
            backlinks: Vec::new(),
            search_results: Vec::new(),
            indexed_query: None,
            show_connections: false,
            focus_editor: false,
            last_edit: None,
            light_theme: false,
            create_note_target_dir: None,
            image_cache: std::collections::HashMap::new(),
            create_folder_target: None,
            new_folder_name: String::new(),
            folder_error: String::new(),
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn explorer_opens_folders_and_selects_files_with_spaces_and_unicode() {
        let root = fixture();
        let folder = root.join("Idee e attività");
        fs::create_dir(&folder).unwrap();
        let note = folder.join("Nota, perché.md");
        fs::write(&note, "test").unwrap();
        let command = file_manager_command(&folder).unwrap();
        assert_eq!(command.get_program(), "explorer.exe");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![folder.as_os_str()]
        );
        let command = file_manager_command(&note).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![std::ffi::OsStr::new("/select,"), note.as_os_str()]
        );
        assert!(file_manager_command(&root.join("Missing.md")).is_err());
    }

    #[test]
    fn moves_open_note_and_saves_future_edits_at_the_new_path() {
        let root = fixture();
        let source = root.join("Idea.md");
        let folder = root.join("Projects/Notes");
        fs::create_dir_all(&folder).unwrap();
        fs::write(&source, "original").unwrap();
        let mut app = test_app(root.clone());
        app.notes.push((source.clone(), "original".into()));
        app.open_file(&source);
        app.editor_text = "unsaved edit".into();
        app.move_note(&source, &folder);
        let destination = folder.join("Idea.md");
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "unsaved edit");
        assert_eq!(app.current_file_path.as_ref(), Some(&destination));
        assert_eq!(app.notes[0].0, destination);
        assert!(app.expanded.contains(&root.join("Projects")));
        app.editor_text = "next edit".into();
        assert!(app.save_current_file());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "next edit");
        app.move_note(&destination, &root);
        assert_eq!(app.current_file_path.as_ref(), Some(&source));
        assert_eq!(fs::read_to_string(&source).unwrap(), "next edit");
    }

    #[test]
    fn move_refuses_collisions_and_external_edits_without_losing_data() {
        let root = fixture();
        let source = root.join("Idea.md");
        let folder = root.join("Notes");
        fs::create_dir(&folder).unwrap();
        let destination = folder.join("Idea.md");
        fs::write(&source, "original").unwrap();
        fs::write(&destination, "existing").unwrap();
        let mut app = test_app(root.clone());
        app.open_file(&source);
        app.editor_text = "draft".into();
        app.move_note(&source, &folder);
        assert!(app.status.contains("already exists"));
        assert!(move_without_overwrite(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "existing");
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        assert_eq!(app.editor_text, "draft");
        let other = root.join("Other");
        fs::create_dir(&other).unwrap();
        fs::write(&source, "external change").unwrap();
        app.move_note(&source, &other);
        assert!(!other.join("Idea.md").exists());
        assert_eq!(fs::read_to_string(&source).unwrap(), "external change");
        assert_eq!(app.editor_text, "draft");
        assert_eq!(app.current_file_path.as_ref(), Some(&source));
    }

    #[test]
    fn move_rejects_outside_vault_and_handles_same_folder() {
        let root = fixture();
        let outside = fixture();
        let source = root.join("Idea.md");
        fs::write(&source, "keep").unwrap();
        let mut app = test_app(root.clone());
        app.move_note(&source, &outside);
        assert!(app.status.contains("inside this vault"));
        assert!(!outside.join("Idea.md").exists());
        app.move_note(&source, &root);
        assert!(app.status.contains("already in this folder"));
        assert_eq!(fs::read_to_string(&source).unwrap(), "keep");
    }

    fn pointer_frame(
        app: &mut NotesApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.ui(ctx),
        )
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + egui::vec2(5.0, 5.0))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing UI label: {label}"))
    }

    #[test]
    fn explorer_rows_stay_left_aligned_and_do_not_jump_during_refresh() {
        let root = fixture();
        let folder = root.join("Folder");
        let mut app = test_app(root.clone());
        let long_name = "A very long note title that must stay on one single explorer row.md";
        app.file_tree.children = Some(vec![FileNode {
            path: folder.clone(),
            is_dir: true,
            children: Some(vec![
                FileNode {
                    path: folder.join("A.md"),
                    is_dir: false,
                    children: None,
                },
                FileNode {
                    path: folder.join(long_name),
                    is_dir: false,
                    children: None,
                },
            ]),
        }]);
        app.expanded.insert(folder);
        let ctx = egui::Context::default();
        pointer_frame(&mut app, &ctx, vec![]);
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let short = text_position(&output, "A.md");
        let long = text_position(&output, long_name);
        let parent = text_position(&output, "Folder");
        assert!((short.x - long.x).abs() < 0.1);
        assert!((short.x - parent.x - 14.0).abs() < 0.1);
        assert!(parent.x < 55.0);
        assert!(text_position(&output, "Vault root").x < 25.0);
        assert!(
            (long.y - short.y - EXPLORER_ROW_HEIGHT - ctx.style().spacing.item_spacing.y).abs()
                < 0.1
        );

        let (tx, rx) = mpsc::channel();
        app.scan = Some(rx);
        tx.send(ScanMessage::Preview(empty_tree(root.clone())))
            .unwrap();
        let loading = pointer_frame(&mut app, &ctx, vec![]);
        assert_eq!(text_position(&loading, "A.md"), short);
        assert_eq!(text_position(&loading, long_name), long);

        // Even a full replacement is deferred while a note is being dragged.
        tx.send(ScanMessage::Tree(empty_tree(root.clone())))
            .unwrap();
        egui::DragAndDrop::set_payload(
            &ctx,
            DraggedNote {
                path: root.join("Dragged.md"),
                vault: root,
            },
        );
        let dragging = pointer_frame(&mut app, &ctx, vec![]);
        assert_eq!(text_position(&dragging, "A.md"), short);
        egui::DragAndDrop::clear_payload(&ctx);
        pointer_frame(&mut app, &ctx, vec![]);
        assert!(app.file_tree.children.as_ref().unwrap().is_empty());
    }

    #[test]
    fn pointer_drag_moves_notes_from_tree_and_search_and_escape_cancels() {
        for (search, cancel) in [(false, false), (true, false), (false, true)] {
            let root = fixture();
            let source = root.join("Drag me.md");
            let folder = root.join("Destination");
            fs::create_dir(&folder).unwrap();
            fs::write(&source, "keep me").unwrap();
            let mut app = test_app(root.clone());
            app.file_tree.children = Some(vec![
                empty_tree(folder.clone()),
                FileNode {
                    path: source.clone(),
                    is_dir: false,
                    children: None,
                },
            ]);
            app.notes.push((source.clone(), "keep me".into()));
            if search {
                app.search = "Drag".into();
            }
            let ctx = egui::Context::default();
            pointer_frame(&mut app, &ctx, vec![]);
            let output = pointer_frame(&mut app, &ctx, vec![]);
            let start = text_position(&output, "Drag me.md");
            pointer_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(start)]);
            pointer_frame(
                &mut app,
                &ctx,
                vec![egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            pointer_frame(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(start + egui::vec2(50.0, 0.0))],
            );
            assert!(egui::DragAndDrop::has_payload_of_type::<DraggedNote>(&ctx));
            let output = pointer_frame(&mut app, &ctx, vec![]);
            let target = text_position(&output, "Destination");
            pointer_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(target)]);
            if cancel {
                pointer_frame(
                    &mut app,
                    &ctx,
                    vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            pointer_frame(
                &mut app,
                &ctx,
                vec![egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(source.exists(), cancel);
            assert_eq!(folder.join("Drag me.md").exists(), !cancel);
            assert!(!egui::DragAndDrop::has_any_payload(&ctx));
        }
    }

    #[test]
    fn creates_nested_folders_without_changing_the_open_note() {
        let root = fixture();
        let mut app = test_app(root.clone());
        app.editor_text = "Unsaved draft".into();
        let mut parent = root.clone();
        for name in ["Projects", "Notes", "Ideas"] {
            app.create_folder_target = Some(parent.clone());
            app.new_folder_name = name.into();
            app.create_folder();
            assert!(app.create_folder_target.is_none());
            assert!(app.expanded.contains(&parent));
            parent = parent.join(name);
            assert!(parent.is_dir());
        }
        assert_eq!(app.editor_text, "Unsaved draft");
        let (tx, rx) = mpsc::channel();
        scan_vault(root, Arc::new(AtomicBool::new(false)), tx, false);
        let _ = rx.recv().unwrap();
        let ScanMessage::Tree(tree) = rx.recv().unwrap() else {
            panic!("Missing full tree");
        };
        let mut rows = Vec::new();
        tree_rows(&tree, &app.expanded, &mut rows);
        assert!(rows.iter().any(|node| node.path == parent));
    }

    #[test]
    fn folder_creation_rejects_invalid_names_and_preserves_existing_entries() {
        let root = fixture();
        fs::create_dir(root.join("Existing")).unwrap();
        fs::write(root.join("Existing/Keep.md"), "keep").unwrap();
        fs::write(root.join("File"), "keep").unwrap();
        let mut app = test_app(root.clone());
        for name in [
            "",
            "..",
            "../escape",
            "a/b",
            "a\\b",
            ".hidden",
            "target",
            "Existing",
            "File",
        ] {
            app.create_folder_target = Some(root.clone());
            app.new_folder_name = name.into();
            app.folder_error.clear();
            app.create_folder();
            assert!(!app.folder_error.is_empty(), "{name}");
            assert!(app.create_folder_target.is_some());
        }
        assert_eq!(
            fs::read_to_string(root.join("Existing/Keep.md")).unwrap(),
            "keep"
        );
        assert_eq!(fs::read_to_string(root.join("File")).unwrap(), "keep");
    }

    #[test]
    fn scan_publishes_navigation_before_content_and_skips_generated_files() {
        let root = fixture();
        fs::create_dir_all(root.join("Idee")).unwrap();
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("Idee/Prima.md"), "# Prima\n[[Seconda]]").unwrap();
        fs::write(root.join("Seconda.txt"), "testo").unwrap();
        fs::write(root.join(".obsidian/Esclusa.md"), "config").unwrap();
        fs::write(root.join("target/Esclusa.md"), "build").unwrap();
        fs::write(root.join("foto.png"), "placeholder").unwrap();
        let (tx, rx) = mpsc::channel();
        scan_vault(root.clone(), Arc::new(AtomicBool::new(false)), tx, true);
        assert!(matches!(rx.recv().unwrap(), ScanMessage::Preview(_)));
        let tree = match rx.recv().unwrap() {
            ScanMessage::Tree(tree) => tree,
            _ => panic!("Missing tree"),
        };
        let mut expanded = HashSet::new();
        expanded.insert(root.join("Idee"));
        let mut rows = Vec::new();
        tree_rows(&tree, &expanded, &mut rows);
        assert_eq!(rows.len(), 4);
        match rx.recv().unwrap() {
            ScanMessage::Ready(index) => {
                assert_eq!(index.notes.len(), 2);
                assert_eq!(index.images.len(), 1);
            }
            _ => panic!("Missing index"),
        }
    }

    #[test]
    fn saving_and_switching_preserves_external_changes_and_current_buffer() {
        let root = fixture();
        let first = root.join("Prima.md");
        let second = root.join("Seconda.md");
        fs::write(&first, "originale").unwrap();
        fs::write(&second, "seconda").unwrap();
        let mut app = test_app(root);
        app.open_file(&first);
        app.editor_text = "modifica locale".into();
        assert!(app.save_current_file());
        assert_eq!(fs::read_to_string(&first).unwrap(), "modifica locale");
        fs::write(&first, "modifica esterna").unwrap();
        app.editor_text = "bozza".into();
        app.open_file(&second);
        assert_eq!(app.current_file_path.as_ref(), Some(&first));
        assert_eq!(app.editor_text, "bozza");
        assert_eq!(fs::read_to_string(&first).unwrap(), "modifica esterna");
    }

    #[test]
    fn sidebar_controls_stay_at_top_and_large_lists_are_virtualized() {
        let mut app = test_app(fixture());
        app.file_tree.children = Some(
            (0..10_000)
                .map(|n| FileNode {
                    path: app.workspace_dir.join(format!("Nota-{n:05}.md")),
                    is_dir: false,
                    children: None,
                })
                .collect(),
        );
        app.current_file_path = Some(app.workspace_dir.join("Editor.md"));
        app.file_tree
            .children
            .as_mut()
            .unwrap()
            .insert(0, empty_tree(app.workspace_dir.join("Folder")));
        for size in [egui::vec2(800.0, 600.0), egui::vec2(1280.0, 820.0)] {
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| app.ui(ctx),
                );
                let texts: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(t) => Some(t),
                        _ => None,
                    })
                    .collect();
                let title = texts
                    .iter()
                    .find(|t| t.galley.job.text == "Editor.md")
                    .unwrap();
                assert!(
                    title.pos.y < 30.0,
                    "A global toolbar must not steal editor height"
                );
                assert!(texts
                    .iter()
                    .all(|t| !t.galley.job.text.contains(['▾', '▸'])));
                let new_note = texts.iter().find(|t| t.galley.job.text == "+").unwrap();
                assert!(new_note.pos.y < 60.0 && new_note.pos.x < 360.0);
                let read = texts.iter().find(|t| t.galley.job.text == "Read").unwrap();
                assert!(read.pos.y < 150.0);
                assert!(!texts.iter().any(|t| [
                    "Refresh files",
                    "Open daily note",
                    "Light theme",
                    "Search contents"
                ]
                .contains(&t.galley.job.text.as_str())));
                let rendered_notes = texts
                    .iter()
                    .filter(|t| t.galley.job.text.starts_with("Nota-"))
                    .count();
                assert!(
                    rendered_notes > 0 && rendered_notes < 50,
                    "Rendered {rendered_notes} rows"
                );
            }
        }
    }
    #[test]
    fn startup_lists_notes_without_reading_contents() {
        let root = fixture();
        fs::write(root.join("Unreadable.md"), [0xff, 0xfe, 0xff]).unwrap();
        let (tx, rx) = mpsc::channel();
        scan_vault(root.clone(), Arc::new(AtomicBool::new(false)), tx, false);
        let index = rx
            .into_iter()
            .find_map(|message| match message {
                ScanMessage::Ready(index) => Some(index),
                _ => None,
            })
            .unwrap();
        assert!(!index.content_loaded);
        assert_eq!(
            index.skipped, 0,
            "Startup must not try to decode note content"
        );
        assert_eq!(index.notes.len(), 1);
        assert!(index.notes[0].1.is_empty());
        let (tx, rx) = mpsc::channel();
        scan_vault(root, Arc::new(AtomicBool::new(false)), tx, true);
        let index = rx
            .into_iter()
            .find_map(|message| match message {
                ScanMessage::Ready(index) => Some(index),
                _ => None,
            })
            .unwrap();
        assert!(index.content_loaded);
        assert_eq!(index.skipped, 1);
    }

    #[test]
    #[ignore = "Read-only timing on an explicitly supplied vault"]
    fn benchmark_vault_listing() {
        let root =
            PathBuf::from(std::env::var("SELVA_BENCH_VAULT").expect("Set SELVA_BENCH_VAULT"));
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || scan_vault(root, Arc::new(AtomicBool::new(false)), tx, false));
        let mut first = true;
        for message in rx {
            match message {
                ScanMessage::Preview(_) if first => {
                    println!("First folders: {:?}", started.elapsed());
                    first = false;
                }
                ScanMessage::Ready(index) => {
                    println!(
                        "Full listing: {:?}; notes: {}; content bytes read: 0",
                        started.elapsed(),
                        index.notes.len()
                    );
                    assert!(!index.content_loaded);
                    return;
                }
                ScanMessage::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        panic!("Scan did not complete");
    }
    #[test]
    fn routes_file_types_case_insensitively() {
        for name in ["a.md", "a.MD", "a.markdown"] {
            assert_eq!(open_kind(Path::new(name)), OpenKind::Markdown);
        }
        for name in ["a.json", "a.JSON", "a.yaml", "a.yml", "a.txt"] {
            assert_eq!(open_kind(Path::new(name)), OpenKind::Text);
        }
        for name in ["a.png", "a.JPG", "a.svg", "a.webp", "a.bmp"] {
            assert_eq!(open_kind(Path::new(name)), OpenKind::Image);
        }
        for name in ["a.pdf", "a.docx", "a.xlsx", "a.zip", "a.unknown"] {
            assert_eq!(open_kind(Path::new(name)), OpenKind::External);
        }
    }

    #[test]
    fn explorer_keeps_arbitrary_extensions_without_reading_them() {
        let root = fixture();
        for name in [
            "a.md",
            "b.json",
            "c.yaml",
            "d.txt",
            "e.png",
            "f.pdf",
            "g.docx",
            "h.unknown",
        ] {
            fs::write(root.join(name), [0xff, 0xfe]).unwrap();
        }
        let (tx, rx) = mpsc::channel();
        scan_vault(root, Arc::new(AtomicBool::new(false)), tx, false);
        for event in rx {
            if let ScanMessage::Tree(tree) = event {
                assert_eq!(tree.children.unwrap().len(), 8);
            }
        }
    }

    #[test]
    fn edits_text_formats_and_previews_images_without_changing_bytes() {
        let root = fixture();
        let mut app = test_app(root.clone());
        for name in ["a.json", "b.yaml", "c.yml", "d.txt"] {
            let path = root.join(name);
            fs::write(&path, "first\nsecond").unwrap();
            app.open_file(&path);
            assert_eq!(app.editor_text, "first\nsecond");
            assert!(app.image_view.is_none());
            app.editor_text.push_str("\nthird");
            assert!(app.save_current_file());
            assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\nthird");
        }
        let png = root.join("preview.png");
        image::RgbaImage::new(4, 4).save(&png).unwrap();
        let before = fs::read(&png).unwrap();
        app.open_file(&png);
        assert!(app.image_view.is_some());
        assert!(app.editor_text.is_empty());
        assert!(app.save_current_file());
        assert_eq!(fs::read(&png).unwrap(), before);
    }

    #[test]
    fn gutter_numbers_logical_lines_including_blank_and_wrapped_lines() {
        for numbered in [false, true] {
            let ctx = egui::Context::default();
            let mut text = format!("{}\n\nlast\n", "wrapped words ".repeat(16));
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let result = text_editor(ui, &mut text, Path::new("sample.txt"), numbered);
                        assert!(result.galley.rows.len() > 4);
                    });
                },
            );
            let numbers: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(t)
                        if ["1", "2", "3", "4"].contains(&t.galley.job.text.as_str()) =>
                    {
                        Some(t.galley.job.text.clone())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                numbers,
                if numbered {
                    vec!["1", "2", "3", "4"]
                } else {
                    vec![]
                }
            );
        }
    }
}
