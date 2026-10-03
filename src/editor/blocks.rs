//! Per-kind block visuals: gutters, quote bars, code backgrounds, rules,
//! table grids and image embeds.

use super::doc::{line_content, BlockKind};
use super::layout::{base_size, EditorTheme};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Vec2};

/// Horizontal offset of the text galley inside a block.
pub fn galley_x_offset(kind: BlockKind, text: &str) -> f32 {
    let indent = (text.len() - text.trim_start().len()) as f32 * 7.0;
    match kind {
        BlockKind::ListItem { .. } => indent + 24.0,
        BlockKind::Quote => indent + 18.0,
        _ => 0.0,
    }
}

/// Gutter label for list items: `•` or the ordinal (e.g. `3.`).
pub fn gutter_label(kind: BlockKind, text: &str) -> Option<String> {
    match kind {
        BlockKind::ListItem { ordered, .. } => {
            let trimmed = text.trim_start();
            if ordered {
                let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
                let sep = trimmed[digits.len()..].chars().next().unwrap_or('.');
                Some(format!("{digits}{sep}"))
            } else {
                let bullet = trimmed.chars().next().unwrap_or('-');
                Some(format!("{bullet}"))
            }
        }
        _ => None,
    }
}

/// Whether the line is a `- [ ]` task; `true` when checked.
pub fn task_state(kind: BlockKind, text: &str) -> Option<bool> {
    match kind {
        BlockKind::ListItem { task: true, .. } => {
            let trimmed = text.trim_start();
            let bracket = trimmed.find('[')?;
            let mark = *trimmed.as_bytes().get(bracket + 1)?;
            let close = *trimmed.as_bytes().get(bracket + 2)?;
            if close != b']' {
                return None;
            }
            Some(mark == b'x' || mark == b'X')
        }
        _ => None,
    }
}

pub fn paint_gutter(
    painter: &egui::Painter,
    block_rect: Rect,
    kind: BlockKind,
    text: &str,
    theme: &EditorTheme,
) {
    let indent = (text.len() - text.trim_start().len()) as f32 * 7.0;
    if let Some(label) = gutter_label(kind, text) {
        if task_state(kind, text).is_some() {
            return; // the checkbox is drawn instead
        }
        let y = block_rect.top() + 2.0;
        painter.text(
            egui::pos2(block_rect.left() + indent + 4.0, y),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(base_size(BlockKind::Paragraph) - 1.0),
            theme.muted,
        );
    }
}

/// Box rect for a task checkbox (the caller wires the click).
pub fn task_checkbox_rect(block_rect: Rect, kind: BlockKind, text: &str) -> Option<Rect> {
    task_state(kind, text)?;
    let indent = (text.len() - text.trim_start().len()) as f32 * 7.0;
    let size = 15.0;
    let y = block_rect.top() + 3.0;
    Some(Rect::from_min_size(
        egui::pos2(block_rect.left() + indent + 4.0, y),
        egui::vec2(size, size),
    ))
}

pub fn paint_checkbox(painter: &egui::Painter, rect: Rect, checked: bool, theme: &EditorTheme) {
    painter.rect_stroke(rect, 3.0, egui::Stroke::new(1.4_f32, theme.muted));
    if checked {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 3.0, rect.center().y),
                Pos2::new(rect.center().x - 1.0, rect.bottom() - 3.5),
            ],
            egui::Stroke::new(2.0_f32, theme.link),
        );
        painter.line_segment(
            [
                Pos2::new(rect.center().x - 1.0, rect.bottom() - 3.5),
                Pos2::new(rect.right() - 2.5, rect.top() + 2.5),
            ],
            egui::Stroke::new(2.0_f32, theme.link),
        );
    }
}

pub fn paint_quote_bar(painter: &egui::Painter, block_rect: Rect, theme: &EditorTheme) {
    let x = block_rect.left() + 8.0;
    painter.line_segment(
        [
            Pos2::new(x, block_rect.top()),
            Pos2::new(x, block_rect.bottom()),
        ],
        egui::Stroke::new(3.0_f32, theme.muted),
    );
}

pub fn paint_code_bg(painter: &egui::Painter, block_rect: Rect, theme: &EditorTheme) {
    painter.rect_filled(block_rect.shrink2(Vec2::new(0.0, 2.0)), 6.0, theme.code_bg);
}

pub fn paint_rule(painter: &egui::Painter, block_rect: Rect, theme: &EditorTheme) {
    let y = block_rect.center().y;
    painter.line_segment(
        [
            Pos2::new(block_rect.left(), y),
            Pos2::new(block_rect.right(), y),
        ],
        egui::Stroke::new(1.5_f32, theme.muted),
    );
}

/// Image target of an `![[name]]` / `![alt](path)` line, if any.
pub fn image_embed(text: &str) -> Option<String> {
    let trimmed = line_content(text).trim();
    if let Some(rest) = trimmed.strip_prefix("![[") {
        let inner = rest.strip_suffix("]]")?;
        return Some(inner.split('|').next().unwrap_or(inner).trim().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("![") {
        let close = rest.find("](")?;
        let path = rest[close + 2..].strip_suffix(')')?;
        return Some(path.trim().to_string());
    }
    None
}

/// Split a GFM table into rows of cells.
pub fn table_cells(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(line_content)
        .map(|line| {
            let trimmed = line.trim().trim_start_matches('|').trim_end_matches('|');
            trimmed.split('|').map(|c| c.trim().to_string()).collect()
        })
        .collect()
}

/// Table grid metrics: column widths and row height for the given width.
pub fn table_metrics(cells: &[Vec<String>], fonts: &egui::text::Fonts) -> (Vec<f32>, f32) {
    let columns = cells.iter().map(|r| r.len()).max().unwrap_or(1);
    let mut widths = vec![0.0_f32; columns];
    let font = egui::FontId::monospace(base_size(BlockKind::Table) - 1.5);
    for row in cells {
        for (i, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(i) {
                let cell_width = fonts.layout_no_wrap(cell.clone(), font.clone(), Color32::WHITE).rect.width();
                *width = (*width).max(cell_width + 16.0);
            }
        }
    }
    for width in &mut widths {
        *width = width.max(48.0);
    }
    (widths, 24.0)
}

pub fn table_height(cells: &[Vec<String>], row_height: f32) -> f32 {
    cells.len() as f32 * row_height + 8.0
}

pub fn paint_table_grid(
    painter: &egui::Painter,
    origin: Pos2,
    cells: &[Vec<String>],
    widths: &[f32],
    row_height: f32,
    theme: &EditorTheme,
) {
    let total_width: f32 = widths.iter().sum();
    let stroke = egui::Stroke::new(1.0_f32, theme.muted);
    for (r, row) in cells.iter().enumerate() {
        let y0 = origin.y + 4.0 + r as f32 * row_height;
        let y1 = y0 + row_height;
        painter.line_segment(
            [Pos2::new(origin.x, y0), Pos2::new(origin.x + total_width, y0)],
            stroke,
        );
        let mut x = origin.x;
        for (c, cell) in row.iter().enumerate() {
            let width = widths.get(c).copied().unwrap_or(48.0);
            painter.text(
                Pos2::new(x + 8.0, y0 + 4.0),
                egui::Align2::LEFT_TOP,
                cell,
                egui::FontId::monospace(base_size(BlockKind::Table) - 1.5),
                if r == 0 { theme.text } else { theme.muted },
            );
            painter.line_segment(
                [Pos2::new(x, y0), Pos2::new(x, y1)],
                stroke,
            );
            x += width;
        }
        painter.line_segment(
            [Pos2::new(origin.x + total_width, y0), Pos2::new(origin.x + total_width, y1)],
            stroke,
        );
        painter.line_segment([Pos2::new(origin.x, y1), Pos2::new(origin.x + total_width, y1)], stroke);
    }
}
