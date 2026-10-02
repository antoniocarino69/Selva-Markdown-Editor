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

#[derive(Clone, Copy, PartialEq)]
enum SearchMode {
    FileNames,
    NamesAndContents,
    FoldersOnly,
}

/// egui's `Frame::side_top_panel` adds 8pt margins on each side, and the panel
/// width machinery measures the content frame: a panel asked for `W - 16`
/// renders exactly `W` points wide. See `exact_width` call sites.
const PANEL_MARGIN_X: f32 = 16.0;

/// Half-width of the custom panel resize grip around each panel edge.
const RESIZE_GRAB: f32 = 4.0;

fn install_fonts(ctx: &egui::Context) {
    let installed = ctx.data(|d| d.get_temp::<bool>(egui::Id::new("selva_fonts")) == Some(true));
    if installed {
        return;
    }
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "selva-bold".to_owned(),
        egui::FontData::from_static(include_bytes!("assets/Ubuntu-Bold.ttf")),
    );
    fonts.families.insert(
        egui::FontFamily::Name("selva-bold".into()),
        vec!["selva-bold".to_owned()],
    );
    ctx.set_fonts(fonts);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("selva_fonts"), true));
}

/// Folder names render in a real bold weight (egui's `strong()` only changes
/// the text color, not the font weight).
fn folder_label(name: impl Into<String>) -> egui::RichText {
    egui::RichText::new(name)
        .font(egui::FontId::new(
            13.5,
            egui::FontFamily::Name("selva-bold".into()),
        ))
        .strong()
}

fn matching_folders(node: &FileNode, root: &Path, query: &str, out: &mut Vec<PathBuf>) {
    if let Some(children) = &node.children {
        for child in children {
            if child.is_dir {
                let relative = child.path.strip_prefix(root).unwrap_or(&child.path);
                if relative.to_string_lossy().to_lowercase().contains(query) {
                    out.push(child.path.clone());
                }
                matching_folders(child, root, query, out);
            }
        }
    }
}

enum FileAction {
    None,
    Open(PathBuf),
    Delete(PathBuf),
    CreateNote(PathBuf),
    CreateFolder(PathBuf),
    ShowInFileManager(PathBuf),
    Rename(PathBuf),
    MoveNote { source: PathBuf, folder: PathBuf },
    ToggleStar(PathBuf),
    Reveal(PathBuf),
}

struct DraggedNote {
    path: PathBuf,
    vault: PathBuf,
}

fn note_drag_source(response: egui::Response, path: &Path, vault: &Path) -> egui::Response {
    if !matches!(open_kind(path), OpenKind::Markdown | OpenKind::Text) {
        return response;
    }
    // Extend the row's own sense instead of overlaying a second widget on the
    // same rect: an overlay would win the click hit-test and `clicked()` on the
    // row would never fire, making notes unselectable.
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

/// Drop target for notes: highlights while a note hovers, and returns true once
/// the pointer has hovered long enough that the caller should expand a closed
/// folder so its subfolders become valid drop targets too.
fn note_drop_target(
    ui: &egui::Ui,
    response: &egui::Response,
    folder: &Path,
    vault: &Path,
    action: &mut FileAction,
) -> bool {
    let timer_id = response.id.with("drop_hover");
    let hovering = response
        .dnd_hover_payload::<DraggedNote>()
        .filter(|note| note.vault == *vault);
    if hovering.is_none() {
        ui.ctx().data_mut(|data| data.remove::<f64>(timer_id));
        return false;
    }
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

// MoveFileW refuses an existing destination, including a file created after
// validation: the check-then-move race can never overwrite a file.
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

fn file_context_menu(
    ui: &mut egui::Ui,
    path: &Path,
    is_dir: bool,
    is_starred: bool,
    action: &mut FileAction,
) {
    let label = if is_dir {
        "Open folder in file explorer"
    } else {
        "Show in file explorer"
    };
    if ui.button(label).clicked() {
        *action = FileAction::ShowInFileManager(path.to_path_buf());
        ui.close_menu();
    }
    if ui
        .button(if is_starred { "Unstar" } else { "Star" })
        .clicked()
    {
        *action = FileAction::ToggleStar(path.to_path_buf());
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
    if !is_dir
        && ui.button("Rename").clicked() {
            *action = FileAction::Rename(path.to_path_buf());
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
        if is_dir {
            command.arg(path);
        } else {
            // Explorer only selects the file when '/select,<path>' is passed
            // as ONE argument.
            command.arg(format!("/select,{}", path.display()));
        }
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
    search_mode: SearchMode,
    sidebar_width: f32,
    connections_width: f32,
    outline_width: f32,
    preview_width: f32,
    scan: Option<mpsc::Receiver<ScanMessage>>,
    content_requested: bool,
    content_ready: bool,
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
    open_tabs: Vec<PathBuf>,
    active_tab: Option<usize>,
    command_palette_open: bool,
    command_palette_query: String,
    command_palette_selected: usize,
    switcher_open: bool,
    switcher_query: String,
    switcher_selected: usize,
    tags: Vec<(String, Vec<PathBuf>)>,
    active_tag: Option<String>,
    show_tags: bool,
    show_outline: bool,
    starred: HashSet<PathBuf>,
    is_renaming: bool,
    rename_target: Option<PathBuf>,
    rename_name: String,
    /// Inner edges of the visible panels, measured each frame; the resize
    /// grips sit on these (the egui frame margins make nominal widths drift).
    panel_edges: Vec<(&'static str, f32)>,
    /// Files listed in the command palette, in display order; palette entries
    /// carry their index into this list (not into the full notes vector).
    palette_files: Vec<PathBuf>,
    /// External file awaiting user confirmation before launching it.
    pending_external: Option<PathBuf>,
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

fn extract_tags(text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut in_code_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        // Skip headings (lines starting with # after optional whitespace)
        if trimmed.starts_with('#') {
            continue;
        }
        let mut chars = trimmed.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            if ch == '`' {
                // Skip inline code (`end` is a byte offset, the iterator
                // yields characters: convert before consuming).
                if let Some(end) = trimmed[i + 1..].find('`') {
                    let skip = trimmed[i + 1..=i + 1 + end].chars().count();
                    for _ in 0..skip {
                        chars.next();
                    }
                    continue;
                }
            }
            if ch == '#' && (i == 0 || !trimmed.as_bytes()[i - 1].is_ascii_alphanumeric()) {
                let rest = &trimmed[i + 1..];
                let tag: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if !tag.is_empty() && !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
        }
    }
    tags
}

fn extract_headings(text: &str) -> Vec<(usize, String)> {
    let mut headings = Vec::new();
    let mut in_code_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        let level = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&level) && trimmed.as_bytes().get(level) == Some(&b' ') {
            let heading_text = trimmed[level..].trim().to_string();
            if !heading_text.is_empty() {
                headings.push((level, heading_text));
            }
        }
    }
    headings
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
        // Catppuccin Mocha-inspired palette
        let base = egui::Color32::from_rgb(30, 30, 46);       // #1e1e2e
        let mantle = egui::Color32::from_rgb(24, 24, 37);     // #181825
        let crust = egui::Color32::from_rgb(17, 17, 27);      // #11111b
        let surface0 = egui::Color32::from_rgb(49, 50, 68);   // #313244
        let surface1 = egui::Color32::from_rgb(69, 71, 90);   // #45475a
        let overlay0 = egui::Color32::from_rgb(108, 112, 134);// #6c7086
        let text = egui::Color32::from_rgb(205, 214, 244);    // #cdd6f4
        let mauve = egui::Color32::from_rgb(137, 180, 250);   // #89b4fa

        visuals.panel_fill = base;
        visuals.window_fill = surface0;
        visuals.extreme_bg_color = crust;
        visuals.faint_bg_color = mantle;
        visuals.override_text_color = Some(text);
        visuals.selection.bg_fill = egui::Color32::from_rgb(88, 91, 112); // subtle highlight
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, mauve);
        visuals.hyperlink_color = mauve;
        visuals.warn_fg_color = egui::Color32::from_rgb(249, 226, 175); // #f9e2af
        visuals.error_fg_color = egui::Color32::from_rgb(243, 139, 168); // #f38ba8
        visuals.window_stroke = egui::Stroke::new(1.0_f32, surface1);
        visuals.widgets.noninteractive.bg_fill = base;
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, overlay0);
        visuals.widgets.inactive.bg_fill = base;
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(166, 173, 200));
        visuals.widgets.inactive.weak_bg_fill = base;
        visuals.widgets.hovered.bg_fill = surface0;
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, surface1);
        visuals.widgets.active.bg_fill = surface1;
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.open.bg_fill = surface0;
        visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.striped = false;
        visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    } else {
        // Refined warm light theme — soft parchment tones
        let bg = egui::Color32::from_rgb(252, 250, 245);           // warm ivory
        let panel = egui::Color32::from_rgb(247, 244, 238);       // soft linen
        let surface = egui::Color32::from_rgb(235, 231, 223);     // muted sand
        let surface1 = egui::Color32::from_rgb(225, 221, 212);    // deeper sand
        let overlay = egui::Color32::from_rgb(160, 152, 142);     // warm gray
        let accent = egui::Color32::from_rgb(120, 100, 170);      // muted lavender
        let text = egui::Color32::from_rgb(55, 50, 48);           // warm charcoal
        let sidebar = egui::Color32::from_rgb(244, 241, 234);     // distinct sidebar
        visuals.panel_fill = bg;
        visuals.window_fill = panel;
        visuals.extreme_bg_color = sidebar;
        visuals.faint_bg_color = egui::Color32::from_rgb(249, 246, 240);
        visuals.override_text_color = Some(text);
        visuals.selection.bg_fill = egui::Color32::from_rgb(210, 205, 235);
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, accent);
        visuals.hyperlink_color = accent;
        visuals.warn_fg_color = egui::Color32::from_rgb(200, 150, 50);
        visuals.error_fg_color = egui::Color32::from_rgb(190, 70, 80);
        visuals.window_stroke = egui::Stroke::new(1.0_f32, surface);
        visuals.widgets.noninteractive.bg_fill = bg;
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, overlay);
        visuals.widgets.inactive.bg_fill = bg;
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(120, 115, 108));
        visuals.widgets.inactive.weak_bg_fill = panel;
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(240, 236, 228);
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, surface1);
        visuals.widgets.active.bg_fill = surface;
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.open.bg_fill = surface;
        visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.striped = false;
    }
    visuals.window_rounding = egui::Rounding::same(8.0);
    visuals.menu_rounding = egui::Rounding::same(8.0);
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: egui::Vec2::new(0.0, 4.0),
        blur: 12.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(60),
    };
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
        // Must happen before the first frame: font changes apply at the start
        // of the next frame, and folder labels use the bold family immediately.
        install_fonts(&cc.egui_ctx);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.indent = 18.0;
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.5));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(19.0));
        style.visuals.widgets.noninteractive.rounding = egui::Rounding::same(4.0);
        style.visuals.widgets.inactive.rounding = egui::Rounding::same(4.0);
        style.visuals.widgets.hovered.rounding = egui::Rounding::same(4.0);
        style.visuals.widgets.active.rounding = egui::Rounding::same(4.0);
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
            search_mode: SearchMode::FileNames,
            sidebar_width: 250.0,
            connections_width: 220.0,
            outline_width: 200.0,
            preview_width: 360.0,
            scan: None,
            content_requested: false,
            content_ready: false,
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
            open_tabs: Vec::new(),
            active_tab: None,
            command_palette_open: false,
            command_palette_query: String::new(),
            command_palette_selected: 0,
            switcher_open: false,
            switcher_query: String::new(),
            switcher_selected: 0,
            tags: Vec::new(),
            active_tag: None,
            show_tags: true,
            show_outline: false,
            starred: HashSet::new(),
            is_renaming: false,
            rename_target: None,
            rename_name: String::new(),
            panel_edges: Vec::new(),
            palette_files: Vec::new(),
            pending_external: None,
        };
        if let Some(storage) = cc.storage {
            if let Some(saved) = eframe::get_value::<HashSet<PathBuf>>(storage, "starred") {
                app.starred = saved;
            }
            for (key, width) in [
                ("sidebar_width", &mut app.sidebar_width),
                ("connections_width", &mut app.connections_width),
                ("outline_width", &mut app.outline_width),
                ("preview_width", &mut app.preview_width),
            ] {
                if let Some(saved) = eframe::get_value::<f32>(storage, key) {
                    if saved.is_finite() && (64.0..=2000.0).contains(&saved) {
                        *width = saved;
                    }
                }
            }
            if let Some(tabs) = eframe::get_value::<Vec<PathBuf>>(storage, "open_tabs") {
                app.open_tabs = tabs;
            }
            if let Some(active) = eframe::get_value::<Option<usize>>(storage, "active_tab") {
                app.active_tab = active;
            }
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
                // Never launch a vault file silently: a synced vault could
                // contain an executable. Ask first.
                self.pending_external = Some(path.to_path_buf());
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
                // Manage tabs only for files that actually opened.
                let path_buf = path.to_path_buf();
                if let Some(idx) = self.open_tabs.iter().position(|p| p == &path_buf) {
                    self.active_tab = Some(idx);
                } else {
                    self.open_tabs.push(path_buf);
                    self.active_tab = Some(self.open_tabs.len() - 1);
                }
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
        for tab in &mut self.open_tabs {
            if tab == source {
                *tab = destination.clone();
            }
        }
        if self.starred.remove(source) {
            self.starred.insert(destination.clone());
        }
        for ancestor in folder.ancestors() {
            if ancestor.starts_with(&self.workspace_dir) {
                self.expanded.insert(ancestor.to_path_buf());
            }
        }
        self.indexed_query = None;
        self.refresh_backlinks();
        self.refresh();
        self.status = "Note moved.".into();
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
        // Keep drop targets under the pointer stable until the drag has finished.
        if egui::DragAndDrop::has_payload_of_type::<DraggedNote>(ctx) {
            return;
        }
        if let Some(rx) = &self.scan {
            match rx.try_recv() {
                Ok(ScanMessage::Preview(tree)) => {
                    // A shallow startup preview must not collapse a populated tree.
                    if self
                        .file_tree
                        .children
                        .as_ref()
                        .is_none_or(|children| children.is_empty())
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
                    self.refresh_tags();
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

    fn refresh_tags(&mut self) {
        let mut tag_map: std::collections::HashMap<String, Vec<PathBuf>> =
            std::collections::HashMap::new();
        for (path, text) in &self.notes {
            for tag in extract_tags(text) {
                tag_map.entry(tag).or_default().push(path.clone());
            }
        }
        let mut tags: Vec<(String, Vec<PathBuf>)> = tag_map.into_iter().collect();
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        self.tags = tags;
    }

    fn update_wiki_links(&mut self, old_name: &str, new_name: &str) {
        // Update wiki-links in all notes that reference the old name
        for (path, text) in &mut self.notes {
            let is_current = self.current_file_path.as_deref() == Some(path.as_path());
            // Rewrite from the live buffer for the open note so unsaved edits
            // are preserved (and saved_text stays in sync for autosave).
            let updated = if is_current {
                self.editor_text.clone()
            } else {
                text.clone()
            };
            if !updated.contains(old_name) {
                continue;
            }
            let links = wiki_links(&updated);
            if !links.iter().any(|link| link.eq_ignore_ascii_case(old_name)) {
                continue;
            }
            // Find and replace wiki-link references
            let mut result = String::new();
            let mut remaining = updated.as_str();
            while let Some(start) = remaining.find("[[") {
                result.push_str(&remaining[..start]);
                if let Some(end) = remaining[start..].find("]]") {
                    let link_content = &remaining[start + 2..start + end];
                    // Check if this link targets the old name (before the pipe or hash)
                    let target = link_content
                        .split('|').next().unwrap_or("")
                        .split('#').next().unwrap_or("")
                        .trim();
                    if target.eq_ignore_ascii_case(old_name) {
                        // Replace the target while preserving alias/heading.
                        // Locate the target inside the raw link content: its
                        // trimmed form may be offset by leading whitespace.
                        let offset = link_content.find(target).unwrap_or(0);
                        let rest = &link_content[offset + target.len()..];
                        result.push_str(&format!("[[{}{}]]", new_name, rest));
                    } else {
                        result.push_str(&remaining[start..start + end + 2]);
                    }
                    remaining = &remaining[start + end + 2..];
                } else {
                    result.push_str(&remaining[start..]);
                    remaining = "";
                }
            }
            result.push_str(remaining);
            if result != updated {
                if let Err(e) = fs::write(path.as_path(), &result) {
                    self.status = format!("Could not update links in {}: {e}", path.display());
                } else {
                    *text = result.clone();
                    if is_current {
                        self.editor_text = result;
                        self.saved_text = self.editor_text.clone();
                    }
                }
            }
        }
    }

    fn insert_at_cursor(&mut self, text: &str, output: &egui::text_edit::TextEditOutput) {
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
            self.editor_text.replace_range(byte_min..byte_max, text);
        } else {
            self.editor_text.push_str(text);
        }
    }

    fn save_pasted_image(
        &mut self,
        img_data: arboard::ImageData,
        filename: &str,
        output: &egui::text_edit::TextEditOutput,
    ) -> bool {
        let target_dir = self.workspace_dir.join("_assets");
        if let Err(e) = fs::create_dir_all(&target_dir) {
            self.status = format!("Could not save pasted image: {e}");
            return false;
        }
        // Timestamps have one-second granularity: never overwrite an earlier
        // paste, and insert a link to the name actually used on disk.
        let mut file_path = target_dir.join(filename);
        if file_path.exists() {
            let stem = file_path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            for n in 2..10_000 {
                file_path = target_dir.join(format!("{stem} {n}.png"));
                if !file_path.exists() {
                    break;
                }
            }
        }
        let Some(img_buffer) = image::RgbaImage::from_raw(
            img_data.width.try_into().unwrap(),
            img_data.height.try_into().unwrap(),
            img_data.bytes.into_owned(),
        ) else {
            self.status = "Could not decode pasted image.".into();
            return false;
        };
        if img_buffer.save(&file_path).is_err() {
            self.status = "Could not save pasted image.".into();
            return false;
        }
        let saved_name = file_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let insert_text = format!("![[{saved_name}]]");
        self.insert_at_cursor(&insert_text, output);
        self.image_cache.insert(saved_name, file_path);
        true
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
            self.search_mode = SearchMode::FileNames;
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
            self.tags.clear();
            self.active_tag = None;
            self.show_outline = false;
            // Tabs and stars belong to the previous vault: drop them so no
            // entry can open a file outside the newly chosen vault.
            self.open_tabs.clear();
            self.active_tab = None;
            self.starred.retain(|p| p.starts_with(&self.workspace_dir));
            self.file_tree = empty_tree(self.workspace_dir.clone());
            self.refresh();
        }
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

    /// Returns true when the tab was actually closed. A pending save that
    /// fails (external edits) keeps the tab open so no draft is lost.
    fn close_tab(&mut self, index: usize) -> bool {
        if index >= self.open_tabs.len() {
            return false;
        }
        if !self.save_current_file() {
            return false;
        }
        self.open_tabs.remove(index);
        if self.open_tabs.is_empty() {
            self.active_tab = None;
            self.current_file_path = None;
            self.image_view = None;
            self.editor_text.clear();
            self.saved_text.clear();
            self.last_edit = None;
        } else {
            let new_active = if let Some(active) = self.active_tab {
                if active >= self.open_tabs.len() {
                    Some(self.open_tabs.len() - 1)
                } else if active > index {
                    Some(active - 1)
                } else {
                    Some(active)
                }
            } else {
                Some(0)
            };
            self.active_tab = new_active;
            if let Some(idx) = new_active {
                let path = self.open_tabs[idx].clone();
                self.open_file(&path);
            }
        }
        true
    }

    fn switch_to_tab(&mut self, index: usize) {
        if index < self.open_tabs.len() {
            let path = self.open_tabs[index].clone();
            self.open_file(&path);
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
        eframe::set_value(storage, "open_tabs", &self.open_tabs);
        eframe::set_value(storage, "active_tab", &self.active_tab);
        eframe::set_value(storage, "starred", &self.starred);
        eframe::set_value(storage, "sidebar_width", &self.sidebar_width);
        eframe::set_value(storage, "connections_width", &self.connections_width);
        eframe::set_value(storage, "outline_width", &self.outline_width);
        eframe::set_value(storage, "preview_width", &self.preview_width);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl NotesApp {
    fn ui(&mut self, ctx: &egui::Context) {
        install_fonts(ctx);
        let mut panel_edges: Vec<(&'static str, f32)> = Vec::new();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
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
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::P)) {
            self.command_palette_open = true;
            self.command_palette_query.clear();
            self.command_palette_selected = 0;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O)) {
            self.switcher_open = true;
            self.switcher_query.clear();
            self.switcher_selected = 0;
        }
        // Ctrl+W close active tab
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W)) {
            if let Some(idx) = self.active_tab {
                self.close_tab(idx);
            }
        }
        // Ctrl+Tab / Ctrl+Shift+Tab for next/prev tab
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Tab)) {
            if let Some(active) = self.active_tab {
                if ctx.input(|i| i.modifiers.shift) {
                    // Previous tab
                    if active > 0 {
                        self.switch_to_tab(active - 1);
                    } else if !self.open_tabs.is_empty() {
                        self.switch_to_tab(self.open_tabs.len() - 1);
                    }
                } else {
                    // Next tab
                    if active + 1 < self.open_tabs.len() {
                        self.switch_to_tab(active + 1);
                    } else if !self.open_tabs.is_empty() {
                        self.switch_to_tab(0);
                    }
                }
            }
        }
        // Ctrl+D: toggle star on current note
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::D)) {
            if let Some(path) = self.current_file_path.clone() {
                if !self.starred.remove(&path) {
                    self.starred.insert(path);
                }
            }
        }
        // Ctrl+Shift+O: toggle outline panel
        if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::O)) {
            self.show_outline = !self.show_outline;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.save_current_file() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        egui::TopBottomPanel::bottom("status")
            .exact_height(22.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let status_text = egui::RichText::new(&self.status)
                        .size(11.5)
                        .color(ui.visuals().widgets.noninteractive.fg_stroke.color);
                    ui.label(status_text);
                    // Word/char count
                    ui.label(
                        egui::RichText::new(format!(
                            "{} words · {} chars",
                            self.editor_text.split_whitespace().count(),
                            self.editor_text.chars().count()
                        ))
                        .size(11.5)
                        .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    if self.editor_text != self.saved_text {
                        ui.label(
                            egui::RichText::new("●")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(249, 226, 175)),
                        );
                        ui.label(
                            egui::RichText::new("Unsaved")
                                .size(11.5)
                                .color(egui::Color32::from_rgb(249, 226, 175)),
                        );
                    }
                });
            });
        let content_y = ctx.available_rect().y_range();
        let mut action = FileAction::None;

        egui::SidePanel::left("vault_sidebar_v2")
            .resizable(false)
            .exact_width(self.sidebar_width - PANEL_MARGIN_X)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 6.0);
                    ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
                    // Vault header — name + actions
                    ui.horizontal(|ui| {
                        let name = self
                            .workspace_dir
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let short = if name.chars().count() > 20 {
                            format!("{}…", name.chars().take(19).collect::<String>())
                        } else {
                            name.clone()
                        };
                        let width = (ui.available_width() - 32.0).max(100.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 28.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.menu_button(
                                    egui::RichText::new(short).size(14.0).strong(),
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
                                [28.0, 28.0],
                                egui::Button::new(egui::RichText::new("+").size(18.0)).frame(false),
                            )
                            .on_hover_text("New note · Ctrl+N")
                            .clicked()
                        {
                            self.is_creating_note = true;
                            self.new_note_name.clear();
                            self.create_note_target_dir = None;
                        }
                    });
                    // Search bar
                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .rounding(6.0)
                        .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let width = (ui.available_width() - 24.0).max(60.0);
                                let search = ui.add(
                                    egui::TextEdit::singleline(&mut self.search)
                                        .frame(false)
                                        .desired_width(width)
                                        .hint_text(match self.search_mode {
                                            SearchMode::NamesAndContents => "Search contents…",
                                            SearchMode::FoldersOnly => "Search folders…",
                                            SearchMode::FileNames => "Search notes…",
                                        }),
                                );
                                if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::K))
                                {
                                    search.request_focus();
                                }
                                ui.menu_button("…", |ui| {
                                    ui.weak("SEARCH IN");
                                    for (mode, label) in [
                                        (SearchMode::FileNames, "File names"),
                                        (SearchMode::NamesAndContents, "Names and contents"),
                                        (SearchMode::FoldersOnly, "Folders only"),
                                    ] {
                                        if ui
                                            .selectable_label(self.search_mode == mode, label)
                                            .clicked()
                                        {
                                            self.search_mode = mode;
                                            if mode == SearchMode::NamesAndContents {
                                                self.content_requested = true;
                                            }
                                            self.indexed_query = None;
                                            ui.close_menu();
                                        }
                                    }
                                })
                                .response
                                .on_hover_text("Search options");
                            });
                        });
                    // Display mode toggle — clean segmented control
                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .rounding(6.0)
                        .inner_margin(2.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 1.0;
                            ui.horizontal(|ui| {
                                let wide = ctx.screen_rect().width() >= 1050.0;
                                let count = if wide { 3.0 } else { 2.0 };
                                let width = (ui.available_width() - (count - 1.0) * 1.0) / count;
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
                                            [width, 24.0],
                                            egui::SelectableLabel::new(selected, label),
                                        )
                                        .clicked()
                                    {
                                        self.display_mode = mode;
                                    }
                                }
                            });
                        });
                    ui.add_space(2.0);
                });
                ui.separator();
                if self.scan.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(if self.content_requested {
                            "Indexing contents…"
                        } else {
                            "Opening vault…"
                        });
                    });
                }
                if !self.starred.is_empty() && self.search.trim().is_empty() {
                    let _ = ui.selectable_label(false,
                        egui::RichText::new("STARRED").size(11.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    let mut starred_paths: Vec<PathBuf> = self.starred.iter().cloned().collect();
                    starred_paths.sort();
                    starred_paths.truncate(10);
                    let current = self.current_file_path.clone();
                    let mut unstar: Option<PathBuf> = None;
                    let mut reveal: Option<PathBuf> = None;
                    for path in &starred_paths {
                        let is_dir = path.is_dir();
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            let star_btn = ui.add_sized(
                                egui::vec2(16.0, 20.0),
                                egui::Button::new(egui::RichText::new("★").size(12.0)
                                    .color(egui::Color32::from_rgb(230, 190, 60)))
                                    .frame(false),
                            );
                            if star_btn.clicked() {
                                unstar = Some(path.clone());
                            }
                            let label_text = if is_dir {
                                folder_label(name.into_owned())
                            } else {
                                egui::RichText::new(name.into_owned()).size(13.0)
                            };
                            let response = ui.add(egui::SelectableLabel::new(
                                current.as_ref() == Some(path),
                                label_text,
                            ));
                            if is_dir {
                                if response.clicked() {
                                    reveal = Some(path.clone());
                                }
                            } else {
                                let response =
                                    note_drag_source(response, path, &self.workspace_dir);
                                if response.clicked() {
                                    action = FileAction::Open(path.clone());
                                }
                            }
                        });
                    }
                    if let Some(p) = unstar {
                        self.starred.remove(&p);
                    }
                    if let Some(p) = reveal {
                        action = FileAction::Reveal(p);
                    }
                    ui.add_space(2.0);
                    ui.separator();
                }
                // While a note is dragged the folder tree must stay visible as the
                // set of drop targets, even when searching or filtering by tag.
                let show_tree = (self.search.trim().is_empty() && self.active_tag.is_none())
                    || egui::DragAndDrop::has_payload_of_type::<DraggedNote>(ui.ctx());
                let root_response = ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        egui::SelectableLabel::new(false, "Vault root"),
                    )
                    .on_hover_text("Drop a note here to move it to the vault root");
                note_drop_target(
                    ui,
                    &root_response,
                    &self.workspace_dir,
                    &self.workspace_dir,
                    &mut action,
                );
                if !show_tree && self.search.trim().is_empty() {
                    // If a tag is active, show its filtered list instead of the full tree
                    if let Some(active) = &self.active_tag {
                        let tag_paths: Vec<PathBuf> = self.tags.iter()
                            .find(|(name, _)| name == active)
                            .map(|(_, paths)| paths.clone())
                            .unwrap_or_default();
                        ui.small(format!("{} notes with #{}", tag_paths.len(), active));
                        let current = self.current_file_path.clone();
                        egui::ScrollArea::vertical()
                            .id_source("tag_filtered")
                            .show_rows(ui, 26.0, tag_paths.len(), |ui, range| {
                                for index in range {
                                    let path = &tag_paths[index];
                                    let name = path.strip_prefix(&self.workspace_dir)
                                        .unwrap_or(path)
                                        .display()
                                        .to_string();
                                    let response = ui.add(egui::SelectableLabel::new(
                                        current.as_ref() == Some(path),
                                        name,
                                    ));
                                    let response =
                                        note_drag_source(response, path, &self.workspace_dir);
                                    if response.clicked() {
                                        action = FileAction::Open(path.clone());
                                    }
                                    response.context_menu(|ui| {
                                        file_context_menu(
                                            ui,
                                            path,
                                            false,
                                            self.starred.contains(path),
                                            &mut action,
                                        );
                                    });
                                }
                            });
                    }
                } else if show_tree {
                    let mut rows = Vec::new();
                    tree_rows(&self.file_tree, &self.expanded, &mut rows);
                    egui::ScrollArea::vertical()
                        .id_source("vault_files")
                        .auto_shrink([false, false])
                        .drag_to_scroll(false)
                        .show_rows(ui, 26.0, rows.len(), |ui, range| {
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
                                    let is_starred = self.starred.contains(&node.path);
                                    // Bold folder names, regular file names
                                    let label_text = if node.is_dir {
                                        folder_label(label)
                                    } else {
                                        egui::RichText::new(label)
                                    };
                                    let response = ui
                                        .add(egui::SelectableLabel::new(
                                            self.current_file_path.as_ref() == Some(&node.path),
                                            label_text,
                                        ))
                                        .on_hover_text(node.path.display().to_string());
                                    // Drag & drop: notes are draggable, folders are drop targets.
                                    let response = note_drag_source(
                                        response,
                                        &node.path,
                                        &self.workspace_dir,
                                    );
                                    if node.is_dir
                                        && note_drop_target(
                                            ui,
                                            &response,
                                            &node.path,
                                            &self.workspace_dir,
                                            &mut action,
                                        )
                                        && !self.expanded.contains(&node.path)
                                    {
                                        self.expanded.insert(node.path.clone());
                                    }
                                    // Star icon for files and folders
                                    {
                                        let star_text = if is_starred {
                                            egui::RichText::new("★").size(11.0)
                                                .color(egui::Color32::from_rgb(230, 190, 60))
                                        } else {
                                            egui::RichText::new("☆").size(11.0)
                                                .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
                                        };
                                        let star_btn = ui.add_sized(
                                            egui::vec2(14.0, 20.0),
                                            egui::Button::new(star_text).frame(false),
                                        );
                                        if star_btn.clicked() {
                                            if is_starred {
                                                self.starred.remove(&node.path);
                                            } else {
                                                self.starred.insert(node.path.clone());
                                            }
                                        }
                                    }
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
                                        file_context_menu(
                                            ui,
                                            &node.path,
                                            node.is_dir,
                                            is_starred,
                                            &mut action,
                                        );
                                    });
                                });
                            }
                        });
                    let rows_empty = rows.is_empty();
                    drop(rows);
                    if rows_empty && self.scan.is_none() {
                        ui.label("No files in this folder.");
                    }
                } else {
                    let query = self.search.trim().to_lowercase();
                    if self.search_mode == SearchMode::FoldersOnly {
                        if self.indexed_query.as_ref() != Some(&query) {
                            self.search_results.clear();
                            matching_folders(
                                &self.file_tree,
                                &self.workspace_dir,
                                &query,
                                &mut self.search_results,
                            );
                            self.indexed_query = Some(query);
                        }
                        ui.small(format!("{} folders", self.search_results.len()));
                        egui::ScrollArea::vertical()
                            .id_source("search_results")
                            .show_rows(ui, 26.0, self.search_results.len(), |ui, range| {
                                for index in range {
                                    let path = &self.search_results[index];
                                    let is_starred = self.starred.contains(path);
                                    let name = path
                                        .strip_prefix(&self.workspace_dir)
                                        .unwrap_or(path)
                                        .display()
                                        .to_string();
                                    let response =
                                        ui.selectable_label(false, folder_label(name));
                                    if response.clicked() {
                                        action = FileAction::Reveal(path.clone());
                                    }
                                    response.context_menu(|ui| {
                                        file_context_menu(
                                            ui,
                                            path,
                                            true,
                                            is_starred,
                                            &mut action,
                                        );
                                    });
                                }
                            });
                    } else {
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
                                        || (self.search_mode == SearchMode::NamesAndContents
                                            && text.to_lowercase().contains(&query))
                                })
                                .map(|(p, _)| p.clone())
                                .collect();
                            self.indexed_query = Some(query);
                        }
                        ui.small(format!("{} results", self.search_results.len()));
                        egui::ScrollArea::vertical()
                            .id_source("search_results")
                            .show_rows(ui, 26.0, self.search_results.len(), |ui, range| {
                                for index in range {
                                    let path = &self.search_results[index];
                                    let is_starred = self.starred.contains(path);
                                    let response = ui.selectable_label(
                                        self.current_file_path.as_ref() == Some(path),
                                        path.strip_prefix(&self.workspace_dir)
                                            .unwrap_or(path)
                                            .display()
                                            .to_string(),
                                    );
                                    let response =
                                        note_drag_source(response, path, &self.workspace_dir);
                                    if response.clicked() {
                                        action = FileAction::Open(path.clone());
                                    }
                                    response.context_menu(|ui| {
                                        file_context_menu(ui, path, false, is_starred, &mut action);
                                    });
                                }
                            });
                    }
                }
                // Tags section at the bottom of sidebar
                if !self.tags.is_empty() {
                    ui.separator();
                    let tags_header = if self.show_tags { "▾ TAGS" } else { "▸ TAGS" };
                    if ui.selectable_label(false,
                        egui::RichText::new(tags_header).size(11.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    ).clicked() {
                        self.show_tags = !self.show_tags;
                    }
                    if self.show_tags {
                        ui.add_space(2.0);
                        let mut tag_clicked: Option<String> = None;
                        for (tag_name, paths) in &self.tags {
                            let is_active = self.active_tag.as_ref() == Some(tag_name);
                            let pill_text = format!("{} ({})", tag_name, paths.len());
                            let pill = egui::Button::new(
                                egui::RichText::new(&pill_text).size(11.0),
                            )
                            .rounding(egui::Rounding::same(10.0))
                            .fill(if is_active {
                                ui.visuals().selection.bg_fill
                            } else {
                                ui.visuals().extreme_bg_color
                            })
                            .stroke(if is_active {
                                egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color)
                            } else {
                                egui::Stroke::NONE
                            });
                            if ui.add(pill).clicked() {
                                tag_clicked = Some(tag_name.clone());
                            }
                        }
                        if let Some(tag) = tag_clicked {
                            if self.active_tag.as_ref() == Some(&tag) {
                                self.active_tag = None;
                            } else {
                                self.active_tag = Some(tag);
                            }
                        }
                    }
                }
            });
        panel_edges.push(("sidebar", ctx.available_rect().min.x));
        if let Some(parent) = self.create_folder_target.clone() {
            egui::Window::new("New folder")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .title_bar(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new("New folder")
                            .size(15.0)
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(format!("Inside: {}", parent.display()))
                            .size(12.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    ui.add_space(6.0);
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
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked()
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
                .title_bar(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new("New note")
                            .size(15.0)
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Note title")
                            .size(12.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    ui.add_space(6.0);
                    let title = ui.add(
                        egui::TextEdit::singleline(&mut self.new_note_name)
                            .hint_text("An idea to remember")
                            .desired_width(320.0),
                    );
                    if !title.has_focus() && self.new_note_name.is_empty() {
                        title.request_focus();
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked()
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
        if self.is_renaming {
            if let Some(target) = self.rename_target.clone() {
                let ext = target.extension().unwrap_or_default().to_string_lossy().into_owned();
                egui::Window::new("Rename note")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .title_bar(false)
                    .show(ctx, |ui| {
                        ui.label(
                            egui::RichText::new("Rename note")
                                .size(15.0)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        let name_input = ui.add(
                            egui::TextEdit::singleline(&mut self.rename_name)
                                .hint_text("New name")
                                .desired_width(320.0),
                        );
                        if !name_input.has_focus() && self.rename_name.is_empty() {
                            name_input.request_focus();
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let confirm = ui.button("Rename").clicked()
                                || (name_input.has_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                            if confirm && !self.rename_name.is_empty() && valid_note_name(&self.rename_name) {
                                let new_name = if ext.is_empty() {
                                    self.rename_name.clone()
                                } else {
                                    format!("{}.{}", self.rename_name, ext)
                                };
                                let new_path = target.parent().unwrap_or(&target).join(&new_name);
                                if new_path.exists() {
                                    self.status = "A file with that name already exists.".into();
                                } else {
                                    match fs::rename(&target, &new_path) {
                                        Ok(()) => {
                                            // Update wiki-links in all notes
                                            let old_stem = target.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                                            let new_stem = self.rename_name.clone();
                                            if old_stem != new_stem {
                                                self.update_wiki_links(&old_stem, &new_stem);
                                            }
                                            if self.current_file_path.as_ref() == Some(&target) {
                                                self.current_file_path = Some(new_path.clone());
                                            }
                                            // Update open tabs
                                            for tab in &mut self.open_tabs {
                                                if *tab == target {
                                                    *tab = new_path.clone();
                                                }
                                            }
                                            if self.starred.remove(&target) {
                                                self.starred.insert(new_path.clone());
                                            }
                                            self.is_renaming = false;
                                            self.rename_target = None;
                                            self.refresh();
                                            self.status = format!("Renamed to {}", new_name);
                                        }
                                        Err(e) => {
                                            self.status = format!("Could not rename: {e}");
                                        }
                                    }
                                }
                            }
                            if ui.button("Cancel").clicked()
                                || ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                self.is_renaming = false;
                                self.rename_target = None;
                            }
                        });
                    });
            } else {
                self.is_renaming = false;
            }
        }
        if let Some(path) = self.pending_external.clone() {
            egui::Window::new("Open external file")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .title_bar(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new("Open with the default app?")
                            .size(15.0)
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(path.display().to_string())
                            .size(12.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            if let Err(e) = open::that(&path) {
                                self.status = format!("Could not open with the default app: {e}");
                            }
                            self.pending_external = None;
                        }
                        if ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            self.pending_external = None;
                        }
                    });
                });
        }

        match action {
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
                        let was_current = self.current_file_path.as_ref() == Some(&path);
                        self.starred.remove(&path);
                        if let Some(idx) = self.open_tabs.iter().position(|t| *t == path) {
                            self.open_tabs.remove(idx);
                            if self.open_tabs.is_empty() {
                                self.active_tab = None;
                            } else if let Some(active) = self.active_tab {
                                self.active_tab = Some(
                                    if active > idx { active - 1 } else { active }
                                        .min(self.open_tabs.len() - 1),
                                );
                            }
                        }
                        if was_current {
                            self.current_file_path = None;
                            self.image_view = None;
                            self.editor_text.clear();
                            self.saved_text.clear();
                            self.last_edit = None;
                            if let Some(active) = self.active_tab {
                                let next = self.open_tabs[active].clone();
                                self.open_file(&next);
                            }
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
            FileAction::Rename(path) => {
                let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                self.is_renaming = true;
                self.rename_target = Some(path);
                self.rename_name = stem;
            }
            FileAction::MoveNote { source, folder } => self.move_note(&source, &folder),
            FileAction::ToggleStar(path) => {
                if !self.starred.remove(&path) {
                    self.starred.insert(path);
                }
            }
            FileAction::Reveal(path) => {
                self.search.clear();
                self.indexed_query = None;
                self.active_tag = None;
                for ancestor in path.ancestors() {
                    if ancestor.starts_with(&self.workspace_dir) {
                        self.expanded.insert(ancestor.to_path_buf());
                    }
                }
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
                .resizable(false)
                .exact_width(self.connections_width - PANEL_MARGIN_X)
                .show(ctx, |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Connections")
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Connect ideas with [[Note name]]")
                            .size(11.5)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("OUTGOING LINKS")
                            .size(11.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
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
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("BACKLINKS")
                            .size(11.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
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
            panel_edges.push(("connections", ctx.available_rect().max.x));
            if let Some(path) = navigate {
                self.open_file(&path);
                ctx.request_repaint();
                return;
            }
        }

        // Outline/TOC panel on the right side
        if self.show_outline && is_markdown && ctx.screen_rect().width() >= 1050.0 {
            egui::SidePanel::right("outline_panel")
                .resizable(false)
                .exact_width(self.outline_width - PANEL_MARGIN_X)
                .show(ctx, |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Outline")
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let headings = extract_headings(&self.editor_text);
                        if headings.is_empty() {
                            ui.small("No headings found.");
                        }
                        for (level, text) in &headings {
                            let indent = (*level - 1) as f32 * 12.0;
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                let size = match *level {
                                    1 => 13.5,
                                    2 => 12.5,
                                    _ => 11.5,
                                };
                                let color = if *level <= 2 {
                                    ui.visuals().text_color()
                                } else {
                                    ui.visuals().widgets.noninteractive.fg_stroke.color
                                };
                                ui.label(
                                    egui::RichText::new(text)
                                        .size(size)
                                        .color(color),
                                );
                            });
                        }
                    });
                });
            panel_edges.push(("outline", ctx.available_rect().max.x));
        }

        // Right panel for preview, ONLY in split mode
        let show_right_preview = is_markdown
            && self.display_mode == DisplayMode::EditAndPreview
            && ctx.screen_rect().width() >= 1050.0;

        if show_right_preview {
            egui::SidePanel::right("preview_panel_v2")
                .resizable(false)
                .exact_width(self.preview_width - PANEL_MARGIN_X)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Preview")
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new("viewer").show(
                            ui,
                            &mut self.commonmark_cache,
                            &processed_text,
                        );
                    });
                });
            panel_edges.push(("preview", ctx.available_rect().max.x));
        }

        // Central panel
        egui::CentralPanel::default().show(ctx, |ui| {
            // Tab bar
            if !self.open_tabs.is_empty() {
                let tab_height = 30.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), tab_height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        egui::ScrollArea::horizontal()
                            .id_source("tab_bar")
                            .max_width(ui.available_width())
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let mut close_idx: Option<usize> = None;
                                    let mut switch_idx: Option<usize> = None;
                                    let tabs_snapshot: Vec<(PathBuf, String)> = self
                                        .open_tabs
                                        .iter()
                                        .map(|p| {
                                            let name = p
                                                .file_stem()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned();
                                            (p.clone(), name)
                                        })
                                        .collect();
                                    for (i, (path, name)) in tabs_snapshot.iter().enumerate() {
                                        let is_active = self.active_tab == Some(i);
                                        let bg = if is_active {
                                            ui.visuals().selection.bg_fill
                                        } else {
                                            egui::Color32::TRANSPARENT
                                        };
                                        let text_color = if is_active {
                                            ui.visuals().strong_text_color()
                                        } else {
                                            ui.visuals().weak_text_color()
                                        };
                                        let tab_frame = egui::Frame::none()
                                            .fill(bg)
                                            .rounding(egui::Rounding {
                                                nw: 4.0,
                                                ne: 4.0,
                                                sw: 0.0,
                                                se: 0.0,
                                            })
                                            .inner_margin(egui::Margin::symmetric(8.0, 4.0));
                                        let tab_response = ui
                                            .allocate_ui_with_layout(
                                                egui::vec2(0.0, tab_height),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    tab_frame.show(ui, |ui| {
                                                        let label = egui::RichText::new(name)
                                                            .size(12.0)
                                                            .color(text_color);
                                                        let r = ui.label(label);
                                                        // x button
                                                        let x_color = ui
                                                            .visuals()
                                                            .widgets
                                                            .inactive
                                                            .fg_stroke
                                                            .color;
                                                        let x_btn = ui.add_sized(
                                                            [16.0, 16.0],
                                                            egui::Button::new(
                                                                egui::RichText::new("×")
                                                                    .size(12.0)
                                                                    .color(x_color),
                                                            )
                                                            .frame(false),
                                                        );
                                                        if x_btn.clicked() {
                                                            close_idx = Some(i);
                                                        }
                                                        r
                                                    })
                                                    .inner
                                                },
                                            )
                                            .inner;
                                        let tab_rect = tab_response.rect;
                                        if tab_response
                                            .interact(egui::Sense::click())
                                            .clicked()
                                        {
                                            switch_idx = Some(i);
                                        }
                                        // Right-click context menu
                                        tab_response.context_menu(|ui| {
                                            if ui.button("Close").clicked() {
                                                close_idx = Some(i);
                                                ui.close_menu();
                                            }
                                            if ui.button("Close Others").clicked() {
                                                // close all except i
                                                let path = self.open_tabs[i].clone();
                                                self.open_tabs.clear();
                                                self.open_tabs.push(path);
                                                self.active_tab = Some(0);
                                                let p = self.open_tabs[0].clone();
                                                self.open_file(&p);
                                                ui.close_menu();
                                            }
                                            if ui.button("Close All").clicked() {
                                                // close remaining (close_tab
                                                // refuses when a save fails)
                                                while !self.open_tabs.is_empty() && self.close_tab(0)
                                                {}
                                                ui.close_menu();
                                            }
                                            if ui.button("Copy Path").clicked() {
                                                ui.output_mut(|o| {
                                                    o.copied_text =
                                                        path.to_string_lossy().into_owned();
                                                });
                                                ui.close_menu();
                                            }
                                        });
                                        // Active tab underline
                                        if is_active {
                                            let painter = ui.painter();
                                            painter.line_segment(
                                                [
                                                    egui::pos2(tab_rect.left(), tab_rect.bottom()),
                                                    egui::pos2(
                                                        tab_rect.right(),
                                                        tab_rect.bottom(),
                                                    ),
                                                ],
                                                egui::Stroke::new(
                                                    2.0_f32,
                                                    ui.visuals().selection.stroke.color,
                                                ),
                                            );
                                        }
                                        ui.add_space(1.0);
                                    }
                                    if let Some(idx) = close_idx {
                                        self.close_tab(idx);
                                    }
                                    if let Some(idx) = switch_idx {
                                        self.switch_to_tab(idx);
                                    }
                                });
                            });
                    },
                );
                ui.separator();
            }
            if let Some(path) = &self.current_file_path {
                let name = path.file_name().unwrap_or_default().to_string_lossy();

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(name.as_ref())
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);

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

                    // Detect image paste before TextEdit processes Ctrl+V.
                    // egui-winit converts Ctrl+V into Event::Paste(text) and
                    // suppresses Event::Key for V. When the clipboard holds an
                    // image (and optionally text), we intercept the Paste event,
                    // check arboard for image data, and remove the event so
                    // TextEdit doesn't also insert the text portion.
                    let mut pending_image: Option<(arboard::ImageData, String)> = None;
                    if is_markdown {
                        let has_paste = ctx.input(|i| {
                            i.events
                                .iter()
                                .any(|e| matches!(e, egui::Event::Paste(_)))
                        });
                        if has_paste {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                if let Ok(img_data) = clipboard.get_image() {
                                    let filename = format!(
                                        "Pasted image {}.png",
                                        chrono::Local::now().format("%Y%m%d%H%M%S")
                                    );
                                    pending_image = Some((img_data, filename));
                                    // Remove Paste event so TextEdit doesn't
                                    // also insert the clipboard text.
                                    ctx.input_mut(|i| {
                                        i.events
                                            .retain(|e| !matches!(e, egui::Event::Paste(_)));
                                    });
                                }
                            }
                        }
                    }

                    let output = egui::ScrollArea::vertical()
                        .id_source(("editor_scroll", path))
                        .show(ui, |ui| {
                            text_editor(ui, &mut self.editor_text, path, !is_markdown)
                        })
                        .inner;

                    // Editor context menu
                    output.response.context_menu(|ui| {
                        if ui.button("Insert code block").clicked() {
                            let insert = "```\n\n```";
                            self.insert_at_cursor(insert, &output);
                            ui.close_menu();
                        }
                        if ui.button("Insert heading").clicked() {
                            self.insert_at_cursor("# ", &output);
                            ui.close_menu();
                        }
                        if ui.button("Insert bullet list").clicked() {
                            self.insert_at_cursor("- ", &output);
                            ui.close_menu();
                        }
                        if ui.button("Insert checkbox").clicked() {
                            self.insert_at_cursor("- [ ] ", &output);
                            ui.close_menu();
                        }
                        if is_markdown {
                            ui.separator();
                            if ui.button("📋 Paste Image").clicked() {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    if let Ok(img_data) = clipboard.get_image() {
                                        let filename = format!(
                                            "Pasted image {}.png",
                                            chrono::Local::now().format("%Y%m%d%H%M%S")
                                        );
                                        if self.save_pasted_image(
                                            img_data,
                                            &filename,
                                            &output,
                                        ) {
                                            self.last_edit = Some(Instant::now());
                                            ctx.request_repaint_after(
                                                Duration::from_millis(700),
                                            );
                                        }
                                    }
                                }
                                ui.close_menu();
                            }
                        }
                    });

                    if self.focus_editor
                        && !self.is_creating_note
                        && self.create_folder_target.is_none()
                    {
                        output.response.request_focus();
                        self.focus_editor = false;
                    }
                    let mut pasted_image = false;
                    if let Some((img_data, filename)) = pending_image {
                        pasted_image = self.save_pasted_image(img_data, &filename, &output);
                    }

                    if output.response.changed() || pasted_image {
                        self.last_edit = Some(Instant::now());
                        ctx.request_repaint_after(Duration::from_millis(700));
                    }
                }
            } else {
                ui.add_space(100.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("✦")
                            .size(36.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Your notes, front and center.")
                            .size(18.0)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Open a note from the sidebar or start writing.")
                            .size(13.0)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                    ui.add_space(24.0);
                    if ui.button("+  Create a note").clicked() {
                        self.is_creating_note = true;
                        self.new_note_name.clear();
                        self.create_note_target_dir = None;
                    }
                    if ui.button("Open another vault").clicked() {
                        self.choose_vault();
                    }
                    ui.add_space(24.0);
                    ui.label(
                        egui::RichText::new("Ctrl+N  New note     Ctrl+K  Search     Ctrl+S  Save     Ctrl+P  Command     Ctrl+O  Open file")
                            .size(11.5)
                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                    );
                });
            }
        }); // end CentralPanel

        // Command Palette overlay (Ctrl+P)
        if self.command_palette_open {
            let query_lower = self.command_palette_query.to_lowercase();
            let commands: Vec<(usize, &str, &str, &str)> = vec![
                (0, "📝", "New Note", "Ctrl+N"),
                (1, "💾", "Save", "Ctrl+S"),
                (2, "🎨", "Toggle Theme", ""),
                (3, "📂", "Open Vault", ""),
                (4, "🔀", "Toggle Split View", ""),
                (5, "📅", "Open Daily Note", ""),
                (6, "🔗", "Toggle Connections", ""),
                (7, "🔄", "Refresh", ""),
                (8, "❌", "Close Tab", "Ctrl+W"),
                (9, "➡️", "Next Tab", "Ctrl+Tab"),
                (10, "⬅️", "Previous Tab", "Ctrl+Shift+Tab"),
                (11, "⭐", "Star Note", "Ctrl+D"),
            ];
            // Add file entries starting at index 12
            let file_entries: Vec<(PathBuf, String)> = self
                .notes
                .iter()
                .filter(|(p, _)| {
                    if query_lower.is_empty() {
                        true
                    } else {
                        p.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query_lower)
                    }
                })
                .take(20)
                .map(|(p, _)| {
                    let name = p
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    (p.clone(), name)
                })
                .collect();
            self.palette_files = file_entries.iter().map(|(p, _)| p.clone()).collect();
            let file_labels: Vec<(usize, String, String)> = file_entries
                .iter()
                .enumerate()
                .map(|(i, (p, name))| {
                    let rel = p
                        .strip_prefix(&self.workspace_dir)
                        .unwrap_or(p)
                        .parent()
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (12 + i, name.clone(), rel)
                })
                .collect();
            // Filter commands
            let mut filtered: Vec<(usize, String, String, String)> = commands
                .iter()
                .filter(|(_, _, label, _)| {
                    query_lower.is_empty() || label.to_lowercase().contains(&query_lower)
                })
                .map(|(i, icon, label, shortcut)| {
                    (*i, icon.to_string(), label.to_string(), shortcut.to_string())
                })
                .collect();
            // Filter files
            let filtered_files: Vec<(usize, String, String, String)> = file_labels
                .iter()
                .filter(|(_, name, _)| {
                    query_lower.is_empty() || name.to_lowercase().contains(&query_lower)
                })
                .map(|(i, name, rel)| (*i, "📄".into(), name.clone(), rel.clone()))
                .collect();
            filtered.extend(filtered_files);
            if self.command_palette_selected >= filtered.len() {
                self.command_palette_selected = filtered.len().saturating_sub(1);
            }
            let item_count = filtered.len();
            egui::Area::new(egui::Id::new("command_palette_overlay"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
                .order(egui::Order::Foreground)
                .interactable(true)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .rounding(10.0)
                        .stroke(egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color))
                        .shadow(ui.visuals().popup_shadow)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.set_min_width(420.0);
                            ui.set_max_width(420.0);
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.command_palette_query)
                                    .hint_text("Type a command…")
                                    .desired_width(400.0)
                                    .frame(false),
                            );
                            if !response.has_focus() {
                                response.request_focus();
                            }
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .show(ui, |ui| {
                                    for (display_idx, item) in filtered.iter().enumerate() {
                                        let (orig_idx, icon, label, shortcut) = item;
                                        let selected = display_idx == self.command_palette_selected;
                                        let bg = if selected {
                                            ui.visuals().selection.bg_fill
                                        } else {
                                            egui::Color32::TRANSPARENT
                                        };
                                        let r = egui::Frame::none()
                                            .fill(bg)
                                            .rounding(4.0)
                                            .inner_margin(egui::Margin::symmetric(6.0, 3.0))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(egui::RichText::new(icon).size(13.0));
                                                    ui.label(egui::RichText::new(label).size(13.0));
                                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                        ui.label(
                                                            egui::RichText::new(shortcut)
                                                                .size(11.0)
                                                                .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                                                        );
                                                    });
                                                });
                                            });
                                        if r.response.interact(egui::Sense::click()).clicked() {
                                            let cmd = *orig_idx;
                                            self.command_palette_open = false;
                                            self.execute_palette_command(cmd, ctx);
                                        }
                                    }
                                });
                            // Keyboard navigation
                            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                                self.command_palette_open = false;
                            }
                            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown))
                                && self.command_palette_selected + 1 < item_count {
                                    self.command_palette_selected += 1;
                                }
                            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp))
                                && self.command_palette_selected > 0 {
                                    self.command_palette_selected -= 1;
                                }
                            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let sel = self.command_palette_selected;
                                if sel < filtered.len() {
                                    let cmd = filtered[sel].0;
                                    self.command_palette_open = false;
                                    self.execute_palette_command(cmd, ctx);
                                }
                            }
                        });
                });
        }

        // Quick Switcher overlay (Ctrl+O)
        if self.switcher_open {
            let query_lower = self.switcher_query.to_lowercase();
            let file_items: Vec<(PathBuf, String, String)> = self
                .notes
                .iter()
                .filter(|(p, _)| {
                    if query_lower.is_empty() {
                        true
                    } else {
                        p.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query_lower)
                    }
                })
                .take(30)
                .map(|(p, _)| {
                    let name = p
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let rel = p
                        .strip_prefix(&self.workspace_dir)
                        .unwrap_or(p)
                        .parent()
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (p.clone(), name, rel)
                })
                .collect();
            if self.switcher_selected >= file_items.len() {
                self.switcher_selected = file_items.len().saturating_sub(1);
            }
            let item_count = file_items.len();
            egui::Area::new(egui::Id::new("quick_switcher_overlay"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
                .order(egui::Order::Foreground)
                .interactable(true)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .rounding(10.0)
                        .stroke(egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color))
                        .shadow(ui.visuals().popup_shadow)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.set_min_width(420.0);
                            ui.set_max_width(420.0);
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.switcher_query)
                                    .hint_text("Type to search files…")
                                    .desired_width(400.0)
                                    .frame(false),
                            );
                            if !response.has_focus() {
                                response.request_focus();
                            }
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .show(ui, |ui| {
                                    for (display_idx, (path, name, rel)) in file_items.iter().enumerate() {
                                        let selected = display_idx == self.switcher_selected;
                                        let bg = if selected {
                                            ui.visuals().selection.bg_fill
                                        } else {
                                            egui::Color32::TRANSPARENT
                                        };
                                        let r = egui::Frame::none()
                                            .fill(bg)
                                            .rounding(4.0)
                                            .inner_margin(egui::Margin::symmetric(6.0, 3.0))
                                            .show(ui, |ui| {
                                                ui.label(egui::RichText::new(name).size(13.0));
                                                if !rel.is_empty() {
                                                    ui.label(
                                                        egui::RichText::new(rel)
                                                            .size(11.0)
                                                            .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                                                    );
                                                }
                                            });
                                        if r.response.interact(egui::Sense::click()).clicked() {
                                            self.switcher_open = false;
                                            self.open_file(path);
                                        }
                                    }
                                });
                            // Keyboard navigation
                            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                                self.switcher_open = false;
                            }
                            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown))
                                && self.switcher_selected + 1 < item_count {
                                    self.switcher_selected += 1;
                                }
                            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp))
                                && self.switcher_selected > 0 {
                                    self.switcher_selected -= 1;
                                }
                            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let sel = self.switcher_selected;
                                if sel < file_items.len() {
                                    let path = file_items[sel].0.clone();
                                    self.switcher_open = false;
                                    self.open_file(&path);
                                }
                            }
                        });
                });
        }
        // A drop target consumes the payload on release; anything left was
        // released outside a target and must not linger as a phantom drag.
        if egui::DragAndDrop::has_any_payload(ctx) && ctx.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ctx);
        }
        self.panel_edges = panel_edges.clone();
        self.resize_handles(ctx, &panel_edges, content_y);
    }

    /// App-owned resize grips on the panel edges. Registered at the very end of
    /// the frame so they win the hit-test against any content underneath, and
    /// driving widths held by the app (egui 0.27's built-in panel resize never
    /// sticks: the panel state stores the content frame rect, so dragged widths
    /// collapse back on the next frame).
    fn resize_handles(
        &mut self,
        ctx: &egui::Context,
        edges: &[(&'static str, f32)],
        y: egui::Rangef,
    ) {
        let screen = ctx.screen_rect();
        let ui = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::background(),
            egui::Id::new("selva_resize_handles"),
            screen,
            screen,
        );
        for (name, edge_x) in edges {
            let rect = egui::Rect::from_x_y_ranges(
                (edge_x - RESIZE_GRAB)..=(edge_x + RESIZE_GRAB),
                y,
            );
            let response =
                ui.interact(rect, egui::Id::new(("selva_resize", name)), egui::Sense::drag());
            if response.hovered() || response.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                ctx.layer_painter(egui::LayerId::background()).vline(
                    *edge_x,
                    y,
                    egui::Stroke::new(2.0_f32, ctx.style().visuals.selection.stroke.color),
                );
            }
            if response.dragged() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let delta = pointer.x - *edge_x;
                    let width = match *name {
                        "sidebar" => &mut self.sidebar_width,
                        "connections" => &mut self.connections_width,
                        "outline" => &mut self.outline_width,
                        _ => &mut self.preview_width,
                    };
                    // The sidebar grows rightwards, the right panels leftwards.
                    let sign = if *name == "sidebar" { 1.0 } else { -1.0 };
                    *width = (*width + sign * delta).clamp(160.0, 2000.0);
                }
            }
        }
    }

    fn execute_palette_command(&mut self, index: usize, ctx: &egui::Context) {
        match index {
            0 => {
                // New Note
                self.is_creating_note = true;
                self.new_note_name.clear();
                self.create_note_target_dir = None;
            }
            1 => {
                // Save
                self.save_current_file();
            }
            2 => {
                // Toggle Theme
                self.light_theme = !self.light_theme;
                apply_theme(ctx, self.light_theme);
            }
            3 => {
                // Open Vault
                self.choose_vault();
            }
            4 => {
                // Toggle Split View
                self.display_mode = match self.display_mode {
                    DisplayMode::EditOnly => DisplayMode::ViewOnly,
                    DisplayMode::ViewOnly => DisplayMode::EditAndPreview,
                    DisplayMode::EditAndPreview => DisplayMode::EditOnly,
                };
            }
            5 => {
                // Open Daily Note
                let name = chrono::Local::now().format("%Y-%m-%d").to_string();
                let path = self.workspace_dir.join(format!("{name}.md"));
                if path.exists() {
                    self.open_file(&path);
                } else {
                    self.new_note_name = name;
                    self.create_note_target_dir = None;
                    self.create_note();
                }
            }
            6 => {
                // Toggle Connections
                self.show_connections = !self.show_connections;
                if self.show_connections {
                    self.content_requested = true;
                }
            }
            7 => {
                // Refresh
                self.refresh();
            }
            8 => {
                // Close Tab
                if let Some(idx) = self.active_tab {
                    self.close_tab(idx);
                }
            }
            9 => {
                // Next Tab
                if let Some(active) = self.active_tab {
                    if active + 1 < self.open_tabs.len() {
                        self.switch_to_tab(active + 1);
                    } else if !self.open_tabs.is_empty() {
                        self.switch_to_tab(0);
                    }
                }
            }
            10 => {
                // Previous Tab
                if let Some(active) = self.active_tab {
                    if active > 0 {
                        self.switch_to_tab(active - 1);
                    } else if !self.open_tabs.is_empty() {
                        self.switch_to_tab(self.open_tabs.len() - 1);
                    }
                }
            }
            11 => {
                // Star Note
                if let Some(path) = &self.current_file_path.clone() {
                    if self.starred.contains(path) {
                        self.starred.remove(path);
                    } else {
                        self.starred.insert(path.clone());
                    }
                }
            }
            _ => {
                // File item (index >= 12): position in the palette's own
                // filtered list, not in the full notes vector.
                let path = index
                    .checked_sub(12)
                    .and_then(|i| self.palette_files.get(i))
                    .cloned();
                if let Some(path) = path {
                    self.open_file(&path);
                }
            }
        }
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
            search_mode: SearchMode::FileNames,
            sidebar_width: 250.0,
            connections_width: 220.0,
            outline_width: 200.0,
            preview_width: 360.0,
            scan: None,
            content_requested: false,
            content_ready: false,
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
            open_tabs: Vec::new(),
            active_tab: None,
            command_palette_open: false,
            command_palette_query: String::new(),
            command_palette_selected: 0,
            switcher_open: false,
            switcher_query: String::new(),
            switcher_selected: 0,
            tags: Vec::new(),
            active_tag: None,
            show_tags: false,
            show_outline: false,
            starred: HashSet::new(),
            is_renaming: false,
            rename_target: None,
            rename_name: String::new(),
            panel_edges: Vec::new(),
            palette_files: Vec::new(),
            pending_external: None,
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
        let expected = format!("/select,{}", note.display());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![std::ffi::OsStr::new(expected.as_str())]
        );
        assert!(file_manager_command(&root.join("Missing.md")).is_err());
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
            install_fonts(&ctx);
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
                ScanMessage::Tree(_) if first => {
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

    #[test]
    fn extracts_tags_from_note_content() {
        let text = "This is a note with #rust and #programming tags.\nAlso #rust and #web-dev.";
        let tags = extract_tags(text);
        assert_eq!(tags, vec!["rust", "programming", "web-dev"]);
    }

    #[test]
    fn ignores_tags_in_code_blocks() {
        let text = "Some text #real\n```\n#not-a-tag\n```\nMore #valid";
        let tags = extract_tags(text);
        assert_eq!(tags, vec!["real", "valid"]);
    }

    #[test]
    fn ignores_tags_in_inline_code() {
        let text = "Use `#not-a-tag` but #real works";
        let tags = extract_tags(text);
        assert_eq!(tags, vec!["real"]);
    }

    #[test]
    fn ignores_heading_hashes() {
        let text = "# Heading\n## Sub heading\nSome #tag here";
        let tags = extract_tags(text);
        assert_eq!(tags, vec!["tag"]);
    }

    #[test]
    fn extracts_headings_from_markdown() {
        let text = "# Title\nSome text\n## Section\n### Sub\n```\n# Not a heading\n```";
        let headings = extract_headings(text);
        assert_eq!(
            headings,
            vec![
                (1, "Title".to_string()),
                (2, "Section".to_string()),
                (3, "Sub".to_string()),
            ]
        );
    }

    #[test]
    fn starred_toggle_and_persistence() {
        let root = fixture();
        let note = root.join("Star.md");
        fs::write(&note, "content").unwrap();
        let mut app = test_app(root);
        app.open_file(&note);
        assert!(app.starred.is_empty());
        app.starred.insert(note.clone());
        assert!(app.starred.contains(&note));
        app.starred.remove(&note);
        assert!(app.starred.is_empty());
    }

    fn pointer_frame(
        app: &mut NotesApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        // Fonts apply at the start of the next frame, so install them before
        // the first `run` (NotesApp::new does the same for the real app).
        install_fonts(ctx);
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

    #[test]
    fn clicking_a_file_row_opens_the_note() {
        let root = fixture();
        let note = root.join("Click me.md");
        fs::write(&note, "hello").unwrap();
        let mut app = test_app(root.clone());
        app.file_tree.children = Some(vec![FileNode {
            path: note.clone(),
            is_dir: false,
            children: None,
        }]);
        let ctx = egui::Context::default();
        pointer_frame(&mut app, &ctx, vec![]);
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let start = text_position(&output, "Click me.md");
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
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.current_file_path.as_ref(), Some(&note));
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
    fn tree_stays_stable_during_preview_and_drag() {
        let root = fixture();
        let folder = root.join("Folder");
        let mut app = test_app(root.clone());
        app.file_tree.children = Some(vec![
            FileNode {
                path: folder.clone(),
                is_dir: true,
                children: Some(vec![FileNode {
                    path: folder.join("A.md"),
                    is_dir: false,
                    children: None,
                }]),
            },
            FileNode {
                path: root.join("Dragged.md"),
                is_dir: false,
                children: None,
            },
        ]);
        app.expanded.insert(folder);
        let ctx = egui::Context::default();
        // The scanning indicator joins the layout as soon as a scan is running,
        // so establish the baseline with an active (idle) scan channel.
        let (tx, rx) = mpsc::channel();
        app.scan = Some(rx);
        pointer_frame(&mut app, &ctx, vec![]);
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let before = text_position(&output, "A.md");

        // A shallow startup preview must not collapse a populated tree.
        tx.send(ScanMessage::Preview(empty_tree(root.clone())))
            .unwrap();
        let loading = pointer_frame(&mut app, &ctx, vec![]);
        assert_eq!(text_position(&loading, "A.md"), before);

        // Even a full replacement is deferred while a note is being dragged.
        tx.send(ScanMessage::Tree(empty_tree(root.clone())))
            .unwrap();
        egui::DragAndDrop::set_payload(
            &ctx,
            DraggedNote {
                path: root.join("Dragged.md"),
                vault: root.clone(),
            },
        );
        let dragging = pointer_frame(&mut app, &ctx, vec![]);
        assert_eq!(text_position(&dragging, "A.md"), before);
        egui::DragAndDrop::clear_payload(&ctx);
        pointer_frame(&mut app, &ctx, vec![]);
        assert!(app.file_tree.children.as_ref().unwrap().is_empty());
    }

    fn click(app: &mut NotesApp, ctx: &egui::Context, at: egui::Pos2) {
        pointer_frame(app, ctx, vec![egui::Event::PointerMoved(at)]);
        pointer_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        pointer_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    fn edge_of(app: &NotesApp, name: &str) -> f32 {
        app.panel_edges
            .iter()
            .find(|(edge, _)| *edge == name)
            .map(|(_, x)| *x)
            .unwrap_or_else(|| panic!("no {name} edge"))
    }

    fn drag_edge(app: &mut NotesApp, ctx: &egui::Context, from: f32, to: f32) {
        pointer_frame(app, ctx, vec![egui::Event::PointerMoved(egui::pos2(from, 400.0))]);
        pointer_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(from, 400.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        for step in 1..=4 {
            pointer_frame(
                app,
                ctx,
                vec![egui::Event::PointerMoved(egui::pos2(
                    from + (to - from) * step as f32 / 4.0,
                    400.0,
                ))],
            );
        }
        pointer_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(to, 400.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    #[test]
    fn side_and_preview_panels_resize_by_dragging_their_edge() {
        let root = fixture();
        let note = root.join("Editor.md");
        fs::write(&note, "# Titolo\n\nciao").unwrap();
        let mut app = test_app(root.clone());
        app.file_tree.children = Some(vec![FileNode {
            path: note.clone(),
            is_dir: false,
            children: None,
        }]);
        app.notes.push((note.clone(), "# Titolo\n\nciao".into()));
        app.open_file(&note);
        app.content_ready = true;
        let ctx = egui::Context::default();
        for _ in 0..3 {
            pointer_frame(&mut app, &ctx, vec![]);
        }

        // Sidebar: drag its right edge 60 points to the right.
        let edge = edge_of(&app, "sidebar");
        drag_edge(&mut app, &ctx, edge, edge + 60.0);
        assert!(
            (app.sidebar_width - 310.0).abs() < 1.0,
            "sidebar width is {}",
            app.sidebar_width
        );

        // Preview panel in split mode: drag its left edge 60 points left.
        app.display_mode = DisplayMode::EditAndPreview;
        for _ in 0..2 {
            pointer_frame(&mut app, &ctx, vec![]);
        }
        let edge = edge_of(&app, "preview");
        drag_edge(&mut app, &ctx, edge, edge - 60.0);
        assert!(
            (app.preview_width - 420.0).abs() < 1.0,
            "preview width is {}",
            app.preview_width
        );

        // Connections panel: drag its left edge 40 points right (shrinks).
        app.show_connections = true;
        for _ in 0..2 {
            pointer_frame(&mut app, &ctx, vec![]);
        }
        let edge = edge_of(&app, "connections");
        drag_edge(&mut app, &ctx, edge, edge + 40.0);
        assert!(
            (app.connections_width - 180.0).abs() < 1.0,
            "connections width is {}",
            app.connections_width
        );
    }

    #[test]
    fn folder_names_render_bold_and_notes_do_not() {
        let root = fixture();
        let folder = root.join("Progetti");
        fs::create_dir(&folder).unwrap();
        let note = root.join("Nota.md");
        fs::write(&note, "ciao").unwrap();
        let mut app = test_app(root.clone());
        app.file_tree.children = Some(vec![
            empty_tree(folder),
            FileNode {
                path: note,
                is_dir: false,
                children: None,
            },
        ]);
        let ctx = egui::Context::default();
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let mut folder_bold = false;
        let mut note_bold = false;
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                let font = &text.galley.job.sections[0].format.font_id;
                let is_bold = font.family == egui::FontFamily::Name("selva-bold".into());
                match text.galley.job.text.as_str() {
                    "Progetti" => folder_bold = is_bold,
                    "Nota.md" => note_bold = is_bold,
                    _ => {}
                }
            }
        }
        assert!(folder_bold, "folder names must use the bold font");
        assert!(!note_bold, "notes stay in the regular font");
    }

    #[test]
    fn folder_search_lists_only_folders_and_reveals_them() {
        let root = fixture();
        let folder = root.join("Progetti Rossi");
        fs::create_dir(&folder).unwrap();
        let note = root.join("Progetti segreti.md");
        fs::write(&note, "testo").unwrap();
        let mut app = test_app(root.clone());
        app.file_tree.children = Some(vec![
            empty_tree(folder.clone()),
            FileNode {
                path: note.clone(),
                is_dir: false,
                children: None,
            },
        ]);
        app.notes.push((note, "testo".into()));
        app.search = "progetti".into();
        app.search_mode = SearchMode::FoldersOnly;
        let ctx = egui::Context::default();
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "Progetti Rossi"));
        assert!(!texts.iter().any(|t| t == "Progetti segreti.md"));
        assert!(texts.iter().any(|t| t == "1 folders"));

        // Clicking a folder result clears the filter and reveals it in the tree.
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let pos = text_position(&output, "Progetti Rossi");
        click(&mut app, &ctx, pos);
        assert!(app.search.is_empty());
        assert!(app.expanded.contains(&folder));
    }

    #[test]
    fn starred_folders_show_in_starred_and_reveal_in_tree() {
        let root = fixture();
        let folder = root.join("Archivio");
        fs::create_dir(&folder).unwrap();
        let mut app = test_app(root.clone());
        app.file_tree.children = Some(vec![empty_tree(folder.clone())]);
        app.starred.insert(folder.clone());
        let ctx = egui::Context::default();
        pointer_frame(&mut app, &ctx, vec![]);
        // The STARRED section paints before the tree, so the first "Archivio"
        // text is its entry.
        let output = pointer_frame(&mut app, &ctx, vec![]);
        let pos = text_position(&output, "Archivio");
        click(&mut app, &ctx, pos);
        assert!(app.expanded.contains(&folder));
        assert!(app.starred.contains(&folder));
        assert!(app.search.is_empty());
    }

    #[test]
    fn closing_the_last_tab_preserves_unsaved_edits() {
        let root = fixture();
        let note = root.join("Draft.md");
        fs::write(&note, "originale").unwrap();
        let mut app = test_app(root);
        app.open_file(&note);
        app.editor_text = "bozza non salvata".into();
        assert!(app.close_tab(0));
        assert!(app.open_tabs.is_empty());
        assert_eq!(fs::read_to_string(&note).unwrap(), "bozza non salvata");
    }

    #[test]
    fn closing_a_tab_keeps_it_open_when_the_save_is_blocked() {
        let root = fixture();
        let note = root.join("Busy.md");
        fs::write(&note, "originale").unwrap();
        let mut app = test_app(root);
        app.open_file(&note);
        app.editor_text = "bozza".into();
        fs::write(&note, "cambiamento esterno").unwrap();
        assert!(!app.close_tab(0));
        assert_eq!(app.open_tabs, vec![note.clone()]);
        assert_eq!(app.editor_text, "bozza");
    }

    #[test]
    fn palette_opens_the_highlighted_file_even_when_filtered() {
        let root = fixture();
        let a = root.join("Alfa.md");
        let b = root.join("Beta.md");
        let c = root.join("Gamma uno.md");
        for p in [&a, &b, &c] {
            fs::write(p, "# t").unwrap();
        }
        let mut app = test_app(root.clone());
        app.notes = vec![
            (a, "# t".into()),
            (b, "# t".into()),
            (c.clone(), "# t".into()),
        ];
        app.command_palette_open = true;
        app.command_palette_query = "gamma".into();
        let ctx = egui::Context::default();
        pointer_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.palette_files, vec![c.clone()]);
        app.execute_palette_command(12, &ctx);
        assert_eq!(app.current_file_path.as_ref(), Some(&c));
    }

    #[test]
    fn wiki_link_rewrite_preserves_aliases_and_unsaved_buffers() {
        let root = fixture();
        let first = root.join("Link.md");
        let second = root.join("Altro.md");
        let target = root.join("Idea.md");
        fs::write(&first, "testo").unwrap();
        fs::write(&second, "vedi [[Idea#parte]]").unwrap();
        fs::write(&target, "# Idea").unwrap();
        let mut app = test_app(root.clone());
        app.notes.push((first.clone(), "testo".into()));
        app.notes.push((second.clone(), "vedi [[Idea#parte]]".into()));
        app.notes.push((target, "# Idea".into()));
        app.open_file(&first);
        // Unsaved edit in the open note, with whitespace inside the brackets.
        app.editor_text = "bozza vedi [[ Idea|alias]]".into();
        app.update_wiki_links("Idea", "Nuova");
        assert_eq!(app.editor_text, "bozza vedi [[Nuova|alias]]");
        assert_eq!(app.saved_text, app.editor_text);
        assert_eq!(fs::read_to_string(&first).unwrap(), app.editor_text);
        assert_eq!(fs::read_to_string(&second).unwrap(), "vedi [[Nuova#parte]]");
    }

    #[test]
    fn tags_after_multibyte_inline_code_are_not_dropped() {
        assert_eq!(extract_tags("x `à`#real"), vec!["real"]);
    }

    #[test]
    fn refresh_tags_collects_all_tags() {
        let root = fixture();
        let mut app = test_app(root.clone());
        app.notes = vec![
            (root.join("a.md"), "Hello #rust #code".into()),
            (root.join("b.md"), "More #rust #web".into()),
        ];
        app.refresh_tags();
        assert_eq!(app.tags.len(), 3); // code, rust, web (sorted)
        let rust_entry = app.tags.iter().find(|(name, _)| name == "rust").unwrap();
        assert_eq!(rust_entry.1.len(), 2); // appears in both notes
    }
}
