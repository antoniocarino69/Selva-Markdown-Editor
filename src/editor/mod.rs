//! The WYSIWYG block editor.
//!
//! Architecture (see `docs/WYSIWYG_PLAN.md`):
//! - [`doc`] is a lossless block cache of the raw markdown (the file on disk
//!   stays the single source of truth);
//! - [`inline`] maps raw block text to display text with hidden syntax markers
//!   and a per-character source map;
//! - [`layout`] turns display text into an egui `Galley` and maps caret
//!   positions between source offsets and galley geometry;
//! - [`blocks`] draws the per-kind visuals (gutters, quote bars, code boxes,
//!   tables, images);
//! - [`input`] holds every editing operation (pure document logic);
//! - [`undo`] is the snapshot stack with typing coalescing.
//!
//! This module wires them into `EditorWidget`: per-block galley caching,
//! viewport virtualization, painting and egui event handling.

pub mod blocks;
pub mod doc;
pub mod inline;
pub mod input;
pub mod layout;
pub mod undo;

pub use doc::{Doc, DocPos};

use eframe::egui;
use egui::{Pos2, Rect, Vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct EditorResponse {
    pub changed: bool,
}

/// Rendering context owned by the app (theme flag, vault image index).
pub struct EditorOpts<'a> {
    pub light: bool,
    pub image_cache: &'a HashMap<String, PathBuf>,
    pub workspace: &'a Path,
}

pub(crate) struct CachedBlock {
    pub text: String,
    pub display: inline::DisplayText,
    pub galley: Arc<egui::Galley>,
    /// Rendered grid height when a table is shown as a grid.
    pub grid_height: Option<f32>,
    /// Image URI, encoded PNG bytes and display size.
    pub image: Option<(String, Arc<[u8]>, Vec2)>,
}

fn pad_top(kind: doc::BlockKind) -> f32 {
    match kind {
        doc::BlockKind::Heading(level) if level <= 2 => 12.0,
        doc::BlockKind::Heading(_) => 8.0,
        doc::BlockKind::Code => 8.0,
        doc::BlockKind::Image => 6.0,
        doc::BlockKind::Rule => 0.0,
        _ => 3.0,
    }
}

fn pad_bottom(kind: doc::BlockKind) -> f32 {
    match kind {
        doc::BlockKind::Heading(_) => 5.0,
        doc::BlockKind::Code => 8.0,
        doc::BlockKind::Image => 6.0,
        doc::BlockKind::Rule => 0.0,
        _ => 4.0,
    }
}

pub struct EditorWidget {
    pub(crate) id: egui::Id,
    pub(crate) doc: Doc,
    pub(crate) cursor: DocPos,
    pub(crate) anchor: DocPos,
    pub(crate) undo: undo::Undo,
    pub(crate) preedit: Option<String>,
    pub(crate) dirty: bool,
    pub(crate) scroll_to_cursor: bool,
    pub(crate) preferred_x: Option<f32>,
    pub(crate) cache: Vec<CachedBlock>,
    cache_width: f32,
    cache_light: bool,
    images: HashMap<PathBuf, Option<(String, Arc<[u8]>, Vec2)>>,
    want_focus: bool,
}

impl EditorWidget {
    pub fn new(id: egui::Id) -> Self {
        Self {
            id,
            doc: Doc::parse(""),
            cursor: DocPos::default(),
            anchor: DocPos::default(),
            undo: undo::Undo::default(),
            preedit: None,
            dirty: false,
            scroll_to_cursor: false,
            preferred_x: None,
            cache: Vec::new(),
            cache_width: -1.0,
            cache_light: false,
            images: HashMap::new(),
            want_focus: false,
        }
    }

    /// (Re)load the document, e.g. when the app opens another note or switches
    /// display mode. Resets the undo history and the caret.
    pub fn load(&mut self, text: &str) {
        self.doc = Doc::parse(text);
        self.cursor = self.doc.pos_start();
        self.anchor = self.cursor;
        self.undo.clear();
        self.preedit = None;
        self.cache.clear();
        self.images.clear();
        self.preferred_x = None;
        self.scroll_to_cursor = false;
        self.dirty = false;
    }

    pub fn text(&self) -> String {
        self.doc.text()
    }

    /// Ask for keyboard focus on the next frame (used when a note is opened).
    pub fn request_focus(&mut self) {
        self.want_focus = true;
    }

    /// Insert raw markdown at the caret (used by the app for pasted images).
    pub fn insert_at_cursor(&mut self, text: &str) {
        self.insert_with(text, undo::EditKind::Structural);
    }

    // -----------------------------------------------------------------------
    // cache

    fn ensure_cache(&mut self, ui: &egui::Ui, width: f32, opts: &EditorOpts) {
        if (self.cache_width - width).abs() > 0.5 || self.cache_light != opts.light {
            self.cache.clear();
            self.cache_width = width;
            self.cache_light = opts.light;
        }
        let theme = layout::EditorTheme::new(&ui.visuals(), opts.light);
        if self.cache.len() != self.doc.blocks.len() {
            self.cache
                .resize_with(self.doc.blocks.len(), || CachedBlock {
                    text: String::new(),
                    display: inline::DisplayText::default(),
                    galley: Arc::new(egui::Galley {
                        job: Arc::new(Default::default()),
                        rows: Vec::new(),
                        elided: false,
                        rect: Rect::NOTHING,
                        mesh_bounds: Rect::NOTHING,
                        num_vertices: 0,
                        num_indices: 0,
                        pixels_per_point: 1.0,
                    }),
                    grid_height: None,
                    image: None,
                });
        }
        let mut decoded_any = false;
        for i in 0..self.doc.blocks.len() {
            let text = self.doc.blocks[i].text.clone();
            if self.cache[i].text == text {
                continue;
            }
            let kind = self.doc.blocks[i].kind;
            let display = inline::parse(&text, kind);
            let wrap = (width - blocks::galley_x_offset(kind, &text) - 12.0).max(80.0);
            let job = layout::block_job(&self.doc.blocks[i], &display, &theme, wrap);
            let galley = ui.fonts(|f| f.layout_job(job));
            let image = if kind == doc::BlockKind::Image {
                let loaded = blocks::image_embed(&text).map(|name| self.image_data(&name, opts));
                match loaded {
                    Some(Some(image)) => {
                        decoded_any = true;
                        Some(image)
                    }
                    _ => None,
                }
            } else {
                None
            };
            let grid_height = if kind == doc::BlockKind::Table {
                let cells = blocks::table_cells(&text);
                let (_, row_height) = ui.fonts(|f| blocks::table_metrics(&cells, f));
                Some(blocks::table_height(&cells, row_height) + pad_top(kind) + pad_bottom(kind))
            } else {
                None
            };
            self.cache[i] = CachedBlock {
                text,
                display,
                galley,
                grid_height,
                image,
            };
        }
        if decoded_any {
            ui.ctx().request_repaint();
        }
    }

    fn image_data(
        &mut self,
        name: &str,
        opts: &EditorOpts,
    ) -> Option<(String, Arc<[u8]>, Vec2)> {
        let path = resolve_image(name, opts);
        let entry = self.images.entry(path.clone()).or_insert_with(|| {
            decode_image(&path).map(|(bytes, size)| {
                (
                    format!(
                        "bytes://selva-wysiwyg-{}",
                        path.to_string_lossy().replace(['\\', ' '], "-")
                    ),
                    bytes,
                    size,
                )
            })
        });
        entry.clone()
    }

    fn block_height(&self, i: usize) -> f32 {
        let Some(cached) = self.cache.get(i) else {
            return 0.0;
        };
        let kind = self.doc.blocks[i].kind;
        if kind == doc::BlockKind::Rule {
            return 22.0;
        }
        if kind == doc::BlockKind::Table
            && self.cursor.block != i
            && self.anchor.block != i
            && !self.table_editing(i)
        {
            if let Some(grid) = cached.grid_height {
                return grid;
            }
        }
        let galley_height = cached.galley.rect.height();
        let image_height = cached
            .image
            .as_ref()
            .map(|(_, _, size)| size.y + 8.0)
            .unwrap_or(0.0);
        pad_top(kind) + galley_height + pad_bottom(kind) + image_height
    }

    fn table_editing(&self, i: usize) -> bool {
        self.cursor.block == i
    }

    /// Document position for a pointer position inside the editor area.
    fn pos_at(&self, pos: Pos2, origin: Pos2, heights: &[f32]) -> DocPos {
        let mut y = origin.y;
        for (i, height) in heights.iter().enumerate() {
            if pos.y <= y + *height || i + 1 == heights.len() {
                let kind = self.doc.blocks[i].kind;
                if kind == doc::BlockKind::Rule {
                    return DocPos::new(i, self.doc.blocks[i].content_start());
                }
                let Some(cached) = self.cache.get(i) else {
                    return DocPos::new(i, self.doc.blocks[i].content_start());
                };
                let x_offset = blocks::galley_x_offset(kind, &cached.text);
                let local = Vec2::new(
                    pos.x - origin.x - x_offset,
                    pos.y - y - pad_top(kind) - image_lift(cached),
                );
                return DocPos::new(
                    i,
                    layout::offset_from_pos(&cached.galley, &cached.display, local),
                );
            }
            y += *height;
        }
        self.doc.pos_end()
    }

    /// Start/end source offsets of the caret's visual row (Home/End).
    fn row_bounds(&self, bi: usize, disp: &inline::DisplayText) -> Option<(usize, usize)> {
        let cached = self.cache.get(bi)?;
        let caret = layout::caret_rect(&cached.galley, disp, self.cursor.off);
        let y = caret.center().y;
        let starts = layout::row_start_chars(&cached.galley);
        for (row_index, row) in cached.galley.rows.iter().enumerate() {
            if row.min_y() <= y && y <= row.max_y() {
                let start = starts[row_index];
                let end = start + row.char_count_excluding_newline();
                return Some((
                    disp.src_at_disp(start),
                    disp.src_at_disp(end.min(disp.chars.len())),
                ));
            }
        }
        None
    }

    /// Up/Down with the galley; falls back to the neighbouring block.
    pub fn move_vertical(&mut self, direction: i32, extend: bool) {
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        if self.preferred_x.is_none() {
            if let Some(cached) = self.cache.get(bi) {
                let caret = layout::caret_rect(&cached.galley, &disp, self.cursor.off);
                self.preferred_x = Some(caret.left() + blocks::galley_x_offset(self.doc.blocks[bi].kind, &cached.text));
            }
        }
        if let Some(cached) = self.cache.get(bi) {
            let local_x = self.preferred_x.unwrap_or(0.0)
                - blocks::galley_x_offset(self.doc.blocks[bi].kind, &cached.text);
            if let Some(off) = layout::vertical_offset(
                &cached.galley,
                &disp,
                self.cursor.off,
                direction,
                local_x,
            ) {
                self.set_cursor(DocPos::new(bi, off), extend);
                self.scroll_to_cursor = true;
                return;
            }
        }
        let target = (bi as i32 + direction).clamp(0, self.doc.blocks.len() as i32 - 1) as usize;
        if target != bi {
            if let Some(cached) = self.cache.get(target) {
                let target_disp = self.block_display(target);
                let x = self.preferred_x.unwrap_or(0.0)
                    - blocks::galley_x_offset(self.doc.blocks[target].kind, &cached.text);
                let y = if direction < 0 {
                    cached.galley.rows.last().map_or(0.0, |row| row.rect.center().y)
                } else {
                    cached.galley.rows.first().map_or(0.0, |row| row.rect.center().y)
                };
                let off = layout::offset_from_pos(&cached.galley, &target_disp, Vec2::new(x, y));
                self.set_cursor(DocPos::new(target, off), extend);
                self.scroll_to_cursor = true;
            }
        }
    }

    // -----------------------------------------------------------------------
    // egui lifecycle

    pub fn show(&mut self, ui: &mut egui::Ui, opts: &EditorOpts) -> EditorResponse {
        let width = ui.available_width().max(80.0);
        self.ensure_cache(ui, width, opts);
        let heights: Vec<f32> = (0..self.cache.len()).map(|i| self.block_height(i)).collect();
        let total = heights.iter().sum::<f32>() + 48.0;
        let id = self.id;
        let theme = layout::EditorTheme::new(&ui.visuals(), opts.light);
        let mut changed = false;
        let mut copied = String::new();

        egui::ScrollArea::vertical()
            .id_source(id.with("scroll"))
            .auto_shrink([false, false])
            // The editor needs drags for text selection, not for scrolling.
            .drag_to_scroll(false)
            .show_viewport(ui, |ui, viewport| {
                let origin = ui.cursor().min;
                let area = Rect::from_min_size(origin, Vec2::new(width, total));
                ui.advance_cursor_after_rect(area);
                let response = ui.interact(area, id, egui::Sense::click_and_drag());
                let has_focus = ui.memory(|m| m.has_focus(id));

                // Caret placement and focus happen on the press frame: a plain
                // click never becomes a "drag" in egui terms.
                let press_started = response.is_pointer_button_down_on()
                    && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
                if press_started {
                    if let Some(pos) = response.interact_pointer_pos() {
                        response.request_focus();
                        let target = self.pos_at(pos, origin, &heights);
                        self.set_cursor(target, ui.input(|i| i.modifiers.shift));
                        self.preferred_x = None;
                    }
                }
                if response.double_clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let target = self.pos_at(pos, origin, &heights);
                        let disp = self.block_display(target.block);
                        let range = disp.word_range(target.off);
                        self.anchor = DocPos::new(target.block, range.start);
                        self.cursor = DocPos::new(target.block, range.end);
                    }
                }
                if response.dragged() && !response.double_clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        self.set_cursor(self.pos_at(pos, origin, &heights), true);
                    }
                }
                if self.want_focus {
                    response.request_focus();
                    self.want_focus = false;
                }

                if has_focus {
                    ui.memory_mut(|m| {
                        m.set_focus_lock_filter(
                            id,
                            egui::EventFilter {
                                tab: true,
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                escape: true,
                                ..Default::default()
                            },
                        )
                    });
                    copied = self.handle_events(ui);
                }

                // Input can merge/split blocks. Refresh layout before painting:
                // cache indices and geometry must refer to the current document.
                if self.dirty {
                    self.ensure_cache(ui, width, opts);
                    ui.ctx().request_repaint();
                }
                let heights: Vec<f32> = (0..self.cache.len())
                    .map(|i| self.block_height(i))
                    .collect();
                let caret_rect = self.paint(ui, origin, &viewport, &heights, &theme, has_focus);
                if has_focus {
                    ui.output_mut(|o| {
                        o.ime = Some(egui::output::IMEOutput {
                            rect: area,
                            cursor_rect: caret_rect.unwrap_or(Rect::from_min_size(
                                area.min,
                                Vec2::new(1.0, 18.0),
                            )),
                        });
                        o.mutable_text_under_cursor = true;
                    });
                }
                if self.scroll_to_cursor {
                    if let Some(rect) = caret_rect {
                        ui.scroll_to_rect(rect.expand(8.0), Some(egui::Align::Center));
                    }
                    self.scroll_to_cursor = false;
                }
                changed = self.dirty;
                self.dirty = false;
            });

        if !copied.is_empty() {
            ui.output_mut(|o| o.copied_text = copied);
        }
        EditorResponse { changed }
    }

    /// Keyboard/clipboard events while focused; returns the text to copy out.
    fn handle_events(&mut self, ui: &egui::Ui) -> String {
        use egui::{Event, Key};
        let mut copied = String::new();
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Copy => copied = self.copy_selection(),
                Event::Cut => {
                    copied = self.copy_selection();
                    if self.selection().is_some() {
                        self.record(undo::EditKind::Structural);
                        self.delete_selection();
                    }
                }
                Event::Paste(text) => {
                    let filtered: String = text.replace('\r', "");
                    self.insert_with(&filtered, undo::EditKind::Structural);
                }
                Event::CompositionStart => self.preedit = Some(String::new()),
                Event::CompositionUpdate(text) => self.preedit = Some(text),
                Event::CompositionEnd(text) => {
                    self.preedit = None;
                    self.insert_with(&text, undo::EditKind::Typing);
                }
                Event::Text(text) => {
                    if ui.input(|i| i.modifiers.command) || self.preedit.is_some() {
                        continue;
                    }
                    let filtered: String =
                        text.chars().filter(|c| *c != '\r' && *c != '\u{0}').collect();
                    if !filtered.is_empty() {
                        self.insert_text(&filtered);
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let word = modifiers.ctrl || modifiers.alt;
                    if modifiers.command {
                        match key {
                            Key::B => self.toggle_wrap("**"),
                            Key::I => self.toggle_wrap("*"),
                            Key::E => self.toggle_wrap("`"),
                            Key::K => self.insert_link(),
                            Key::Z if modifiers.shift => self.redo(),
                            Key::Z => self.undo(),
                            Key::Y => self.redo(),
                            Key::A => self.select_all(),
                            Key::C => copied = self.copy_selection(),
                            Key::X => {
                                copied = self.copy_selection();
                                if self.selection().is_some() {
                                    self.record(undo::EditKind::Structural);
                                    self.delete_selection();
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match key {
                            Key::Enter => self.enter(),
                            Key::Backspace => self.backspace(),
                            Key::Delete => self.delete_forward(),
                            Key::ArrowLeft => {
                                self.move_left(modifiers.shift, word);
                                self.preferred_x = None;
                            }
                            Key::ArrowRight => {
                                self.move_right(modifiers.shift, word);
                                self.preferred_x = None;
                            }
                            Key::ArrowUp => self.move_vertical(-1, modifiers.shift),
                            Key::ArrowDown => self.move_vertical(1, modifiers.shift),
                            Key::Home => {
                                self.home(modifiers.shift);
                                self.preferred_x = None;
                            }
                            Key::End => {
                                self.end(modifiers.shift);
                                self.preferred_x = None;
                            }
                            Key::PageUp => self.move_blocks(-8, modifiers.shift),
                            Key::PageDown => self.move_blocks(8, modifiers.shift),
                            Key::Tab => self.indent(modifiers.shift),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        copied
    }

    /// Paint the visible blocks; returns the caret rect (screen coords).
    fn paint(
        &mut self,
        ui: &mut egui::Ui,
        origin: Pos2,
        viewport: &Rect,
        heights: &[f32],
        theme: &layout::EditorTheme,
        has_focus: bool,
    ) -> Option<Rect> {
        let painter = ui.painter().with_clip_rect(Rect::from_min_max(
            Pos2::new(ui.clip_rect().left(), viewport.min.y - 200.0),
            Pos2::new(ui.clip_rect().right(), viewport.max.y + 200.0),
        ));
        let selection = self.selection();
        let blink_on = has_focus && ((ui.input(|i| i.time) * 2.0).fract() < 0.55);
        if has_focus {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(450));
        }
        let mut caret_rect: Option<Rect> = None;
        let mut task_clicked: Option<usize> = None;
        let mut y = origin.y;
        for i in 0..self.cache.len() {
            let height = heights[i];
            let top = y;
            let bottom = y + height;
            y = bottom;
            if bottom < viewport.min.y - 120.0 {
                continue;
            }
            if top > viewport.max.y + 120.0 {
                break;
            }
            let block = &self.doc.blocks[i];
            let kind = block.kind;
            let cached = &self.cache[i];
            let text = cached.text.clone();
            let x_offset = blocks::galley_x_offset(kind, &text);
            let galley_origin = Pos2::new(
                origin.x + x_offset,
                top + pad_top(kind) + image_lift(cached),
            );
            let block_rect = Rect::from_min_max(
                Pos2::new(origin.x, top),
                Pos2::new(origin.x + self.cache_width, bottom),
            );

            // selection highlight for this block
            let range = selection.and_then(|(a, b)| {
                let content_end = block.content_end();
                if i == a.block && i == b.block {
                    Some((a.off, b.off))
                } else if i == a.block {
                    Some((a.off, content_end))
                } else if i == b.block {
                    Some((block.content_start(), b.off))
                } else if a.block < i && i < b.block {
                    Some((block.content_start(), content_end))
                } else {
                    None
                }
            });
            if let Some((start, end)) = range {
                for rect in layout::selection_rects(&cached.galley, &cached.display, start, end) {
                    painter.rect_filled(rect.translate(galley_origin.to_vec2()), 0.0, theme.selection);
                }
            }

            match kind {
                doc::BlockKind::Rule => {
                    blocks::paint_rule(&painter, block_rect, theme);
                    continue;
                }
                doc::BlockKind::Code => {
                    blocks::paint_code_bg(&painter, block_rect, theme);
                }
                doc::BlockKind::Quote => {
                    blocks::paint_quote_bar(&painter, block_rect, theme);
                }
                doc::BlockKind::ListItem { .. } => {
                    blocks::paint_gutter(&painter, block_rect, kind, &text, theme);
                    if let Some(box_rect) = blocks::task_checkbox_rect(block_rect, kind, &text) {
                        let checked = blocks::task_state(kind, &text).unwrap_or(false);
                        blocks::paint_checkbox(&painter, box_rect, checked, theme);
                        let checkbox = ui.interact(
                            box_rect,
                            self.id.with(("task", i)),
                            egui::Sense::click(),
                        );
                        if checkbox.clicked() {
                            task_clicked = Some(i);
                        }
                    }
                }
                doc::BlockKind::Table if !self.table_editing(i) => {
                    if let Some(grid_height) = cached.grid_height {
                        let cells = blocks::table_cells(&text);
                        let (widths, row_height) = ui.fonts(|f| blocks::table_metrics(&cells, f));
                        blocks::paint_table_grid(
                            &painter,
                            Pos2::new(origin.x, top + pad_top(kind)),
                            &cells,
                            &widths,
                            row_height,
                            theme,
                        );
                        let _ = grid_height;
                        continue;
                    }
                }
                doc::BlockKind::Image => {
                    if let Some((uri, bytes, size)) = &cached.image {
                        let rect = Rect::from_min_size(
                            Pos2::new(origin.x, top + pad_top(kind)),
                            *size * (self.cache_width / size.x).min(1.0),
                        );
                        ui.put(
                            rect,
                            egui::Image::from_bytes(uri.clone(), bytes.clone())
                                .fit_to_exact_size(rect.size()),
                        );
                    }
                }
                _ => {}
            }

            painter.galley(galley_origin, cached.galley.clone(), theme.text);

            if has_focus && self.cursor.block == i {
                let local = layout::caret_rect(&cached.galley, &cached.display, self.cursor.off);
                let absolute = local.translate(galley_origin.to_vec2());
                caret_rect = Some(absolute);
                if blink_on && self.cursor.block == i {
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(absolute.left(), absolute.top()),
                            Vec2::new(1.6, absolute.height().max(10.0)),
                        ),
                        0.0,
                        theme.caret,
                    );
                }
                if let Some(preedit) = &self.preedit {
                    if self.cursor.block == i && !preedit.is_empty() {
                        painter.text(
                            Pos2::new(absolute.left(), absolute.top()),
                            egui::Align2::LEFT_TOP,
                            preedit,
                            egui::FontId::proportional(layout::base_size(kind)),
                            theme.text,
                        );
                    }
                }
            }
        }
        if let Some(i) = task_clicked {
            self.toggle_task(i);
        }
        caret_rect
    }
}

/// Images render above their raw source line.
fn image_lift(cached: &CachedBlock) -> f32 {
    cached
        .image
        .as_ref()
        .map(|(_, _, size)| size.y + 8.0)
        .unwrap_or(0.0)
}

fn resolve_image(name: &str, opts: &EditorOpts) -> PathBuf {
    let direct = PathBuf::from(name);
    if direct.is_absolute() {
        return direct;
    }
    if let Some(path) = opts.image_cache.get(name) {
        return path.clone();
    }
    opts.workspace.join(name.trim_start_matches("./"))
}

fn decode_image(path: &Path) -> Option<(Arc<[u8]>, Vec2)> {
    let image = image::open(path).ok()?.into_rgba8();
    let natural = Vec2::new(image.width() as f32, image.height() as f32);
    // Fit into a 480×400 box, keeping the aspect ratio.
    let scale = (480.0 / natural.x).min(400.0 / natural.y).min(1.0);
    let mut encoded = std::io::Cursor::new(Vec::new());
    image.write_to(&mut encoded, image::ImageFormat::Png).ok()?;
    Some((Arc::from(encoded.into_inner()), natural * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        // Mirror the app's bold family onto default fonts so headings render.
        let mut fonts = egui::FontDefinitions::default();
        let fallbacks = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(egui::FontFamily::Name("selva-bold".into()), fallbacks);
        ctx.set_fonts(fonts);
        ctx
    }

    #[test]
    fn image_payload_is_decodable_and_preserves_pixels() {
        let path = std::env::temp_dir().join(format!("selva-image-test-{}.png", std::process::id()));
        let original = image::RgbaImage::from_pixel(2, 3, image::Rgba([23, 45, 67, 128]));
        original.save(&path).unwrap();
        let decoded = decode_image(&path);
        std::fs::remove_file(&path).unwrap();
        let (bytes, size) = decoded.unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().into_rgba8(), original);
        assert_eq!(size, Vec2::new(2.0, 3.0));
    }

    #[test]
    fn typing_after_caret_movement_has_a_separate_undo_step() {
        let mut widget = EditorWidget::new(egui::Id::new("undo-movement"));
        widget.load("abc\n");
        widget.request_focus();
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        frame(&mut widget, &ctx, vec![egui::Event::Text("X".into())]);
        frame(&mut widget, &ctx, vec![key(egui::Key::End, egui::Modifiers::NONE)]);
        frame(&mut widget, &ctx, vec![egui::Event::Text("Y".into())]);
        assert_eq!(widget.text(), "XabcY\n");
        let command = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        frame(&mut widget, &ctx, vec![key(egui::Key::Z, command)]);
        assert_eq!(widget.text(), "Xabc\n");
        assert_eq!(widget.cursor, DocPos::new(0, 4));
        frame(&mut widget, &ctx, vec![key(egui::Key::Z, command)]);
        assert_eq!(widget.text(), "abc\n");
    }

    #[test]
    fn page_navigation_on_an_empty_note_is_safe() {
        let mut widget = EditorWidget::new(egui::Id::new("empty-page"));
        widget.request_focus();
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        frame(&mut widget, &ctx, vec![key(egui::Key::PageDown, egui::Modifiers::NONE)]);
        frame(&mut widget, &ctx, vec![key(egui::Key::PageUp, egui::Modifiers::NONE)]);
        assert_eq!(widget.text(), "");
    }

    fn frame(
        widget: &mut EditorWidget,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let images = HashMap::new();
        let workspace = Path::new(".");
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    widget.show(
                        ui,
                        &EditorOpts {
                            light: false,
                            image_cache: &images,
                            workspace,
                        },
                    );
                });
            },
        )
    }

    fn text_pos(output: &egui::FullOutput, label: &str) -> Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => Some(text.pos),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing text {label:?}"))
    }

    /// Exact position of a caret gap in the painted display galley.
    fn text_gap(output: &egui::FullOutput, label: &str, ccursor: usize) -> Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    let caret = text
                        .galley
                        .pos_from_ccursor(egui::text::CCursor::new(ccursor));
                    Some(text.pos + caret.left_top().to_vec2())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing text {label:?}"))
    }

    fn click(widget: &mut EditorWidget, ctx: &egui::Context, at: Pos2) {
        frame(widget, ctx, vec![egui::Event::PointerMoved(at)]);
        frame(
            widget,
            ctx,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        frame(
            widget,
            ctx,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn click_places_the_caret_and_typing_edits_that_spot() {
        let mut widget = EditorWidget::new(egui::Id::new("test"));
        widget.load("ciao mondo\n");
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let start = text_pos(&output, "ciao mondo");
        click(&mut widget, &ctx, start + Vec2::new(6.0, 8.0));
        frame(&mut widget, &ctx, vec![egui::Event::Text("X".into())]);
        let text = widget.text();
        assert_eq!(text.chars().count(), 12);
        assert!(text.contains('X'), "typing did not reach the editor: {text:?}");
        assert_eq!(text.replace('X', ""), "ciao mondo\n");
    }

    #[test]
    fn enter_splits_and_typing_continues() {
        let mut widget = EditorWidget::new(egui::Id::new("test"));
        widget.load("ciao mondo\n");
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let start = text_pos(&output, "ciao mondo");
        click(&mut widget, &ctx, start + Vec2::new(50.0, 8.0));
        frame(&mut widget, &ctx, vec![key(egui::Key::End, egui::Modifiers::NONE)]);
        frame(&mut widget, &ctx, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        frame(&mut widget, &ctx, vec![egui::Event::Text("x".into())]);
        assert_eq!(widget.text(), "ciao mondo\nx\n");
    }

    #[test]
    fn hidden_markers_are_not_clickable() {
        let mut widget = EditorWidget::new(egui::Id::new("test"));
        widget.load("a**x**b\n");
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        // The display text is `axb`; clicking between `x` and `b` puts the
        // caret after the closing `**` in source coordinates.
        let gap = text_gap(&output, "axb", 2); // between `x` and `b`
        click(&mut widget, &ctx, gap + Vec2::new(-1.0, 8.0));
        frame(&mut widget, &ctx, vec![egui::Event::Text("Z".into())]);
        assert_eq!(widget.text(), "a**x**Zb\n");
    }

    #[test]
    fn shift_arrows_select_and_ctrl_c_copies_raw_markdown() {
        let mut widget = EditorWidget::new(egui::Id::new("test"));
        widget.load("**grasso** e altro\n");
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let start = text_pos(&output, "grasso e altro");
        click(&mut widget, &ctx, start + Vec2::new(2.0, 8.0));
        for _ in 0..6 {
            frame(
                &mut widget,
                &ctx,
                vec![key(
                    egui::Key::ArrowRight,
                    egui::Modifiers {
                        shift: true,
                        ..egui::Modifiers::NONE
                    },
                )],
            );
        }
        let output = frame(
            &mut widget,
            &ctx,
            vec![key(
                egui::Key::C,
                egui::Modifiers {
                    command: true,
                    ..egui::Modifiers::NONE
                },
            )],
        );
        assert_eq!(output.platform_output.copied_text, "grasso");
    }

    #[test]
    fn vertical_navigation_preserves_column_across_short_code_lines() {
        let mut widget = EditorWidget::new(egui::Id::new("vertical-code"));
        widget.load("```text\nabcdefghijk\nx\nabcdefghijk\n```\n");
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let label = widget.cache[0].display.display.clone();
        let start = label.find("abcdefghijk").unwrap() + 8;
        click(&mut widget, &ctx, text_gap(&output, &label, start) + Vec2::new(0.0, 5.0));
        let original = widget.cursor;
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)]);
        assert_eq!(widget.cursor.off, widget.text().find("\nx\n").unwrap() + 2);
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)]);
        assert_eq!(widget.cursor.off, original.off + "abcdefghijk\nx\n".len());
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)]);
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)]);
        assert_eq!(widget.cursor, original);
    }

    #[test]
    fn vertical_navigation_returns_to_column_after_short_wrapped_row() {
        let mut widget = EditorWidget::new(egui::Id::new("vertical-wrap"));
        widget.load(&format!("{}fine\n", "parola ".repeat(20)));
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let cached = &widget.cache[0];
        assert!(cached.galley.rows.len() >= 2);
        let row_index = cached.galley.rows.len() - 2;
        let starts = layout::row_start_chars(&cached.galley);
        let gap = starts[row_index] + cached.galley.rows[row_index].char_count_excluding_newline() - 5;
        let label = cached.display.display.clone();
        click(&mut widget, &ctx, text_gap(&output, &label, gap) + Vec2::new(0.0, 5.0));
        let original = widget.cursor;
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)]);
        assert!(widget.cursor.off > original.off);
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)]);
        assert_eq!(widget.cursor, original);
    }

    #[test]
    fn ime_commit_replaces_selection_once_and_undo_restores_it() {
        let mut widget = EditorWidget::new(egui::Id::new("ime"));
        widget.load("ciao\n");
        widget.request_focus();
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        frame(&mut widget, &ctx, vec![key(egui::Key::ArrowRight, egui::Modifiers::SHIFT)]);
        let selection = (widget.cursor, widget.anchor);
        frame(&mut widget, &ctx, vec![egui::Event::CompositionStart]);
        frame(&mut widget, &ctx, vec![egui::Event::CompositionUpdate("候".into()), egui::Event::Text("候".into())]);
        assert_eq!(widget.text(), "ciao\n");
        frame(&mut widget, &ctx, vec![egui::Event::CompositionEnd("你好è".into())]);
        assert_eq!(widget.text(), "你好èiao\n");
        let command = egui::Modifiers { command: true, ..Default::default() };
        frame(&mut widget, &ctx, vec![key(egui::Key::Z, command)]);
        assert_eq!(widget.text(), "ciao\n");
        assert_eq!((widget.cursor, widget.anchor), selection);
        frame(&mut widget, &ctx, vec![egui::Event::CompositionStart, egui::Event::CompositionUpdate("候".into()), egui::Event::CompositionEnd(String::new())]);
        assert_eq!(widget.text(), "ciao\n");
        assert_eq!((widget.cursor, widget.anchor), selection);
    }

    #[test]
    fn drag_across_blocks_copies_and_replaces_selection() {
        let mut widget = EditorWidget::new(egui::Id::new("drag-blocks"));
        widget.load("prima\nseconda\n");
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let start = text_gap(&output, "prima", 2) + Vec2::new(0.0, 8.0);
        let end = text_gap(&output, "seconda", 3) + Vec2::new(0.0, 8.0);
        frame(&mut widget, &ctx, vec![egui::Event::PointerMoved(start)]);
        frame(&mut widget, &ctx, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }]);
        frame(&mut widget, &ctx, vec![egui::Event::PointerMoved(end)]);
        frame(&mut widget, &ctx, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }]);
        let output = frame(&mut widget, &ctx, vec![egui::Event::Copy]);
        assert_eq!(output.platform_output.copied_text, "ima\nsec");
        frame(&mut widget, &ctx, vec![egui::Event::Text("è".into())]);
        assert_eq!(widget.text(), "prèonda\n");
    }

    #[test]
    fn big_notes_render_only_visible_blocks() {
        let mut widget = EditorWidget::new(egui::Id::new("test"));
        let source = (0..2000).map(|n| format!("riga numero {n}\n")).collect::<String>();
        widget.load(&source);
        let ctx = test_ctx();
        frame(&mut widget, &ctx, vec![]);
        let output = frame(&mut widget, &ctx, vec![]);
        let rendered = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text.starts_with("riga numero") => {
                    Some(())
                }
                _ => None,
            })
            .count();
        assert!(
            rendered > 0 && rendered < 40,
            "rendered {rendered} blocks (virtualization broken)"
        );
    }
}
