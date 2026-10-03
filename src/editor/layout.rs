//! Block → `LayoutJob` and the cursor/caret mapping between raw source offsets
//! (what editing uses) and display positions (what the galley exposes).

use super::doc::{line_content, Block, BlockKind};
use super::inline::{DisplayText, Style};
use eframe::egui;
use egui::text::{LayoutJob, LayoutSection, TextWrapping};
use egui::{Color32, FontFamily, FontId, Galley, Rect, Stroke, Vec2};
use std::sync::OnceLock;

pub const BOLD_FAMILY: &str = "selva-bold";

/// Colors and fonts for the editor, derived from the app visuals.
pub struct EditorTheme {
    pub text: Color32,
    pub muted: Color32,
    pub link: Color32,
    pub code_bg: Color32,
    pub selection: Color32,
    pub caret: Color32,
    pub light: bool,
}

impl EditorTheme {
    pub fn new(visuals: &egui::Visuals, light: bool) -> Self {
        Self {
            text: visuals.override_text_color.unwrap_or(visuals.text_color()),
            muted: visuals.widgets.noninteractive.fg_stroke.color,
            link: visuals.hyperlink_color,
            code_bg: visuals.extreme_bg_color,
            selection: visuals.selection.bg_fill,
            caret: visuals.strong_text_color(),
            light,
        }
    }
}

pub fn heading_size(level: u8) -> f32 {
    match level {
        1 => 28.0,
        2 => 24.0,
        3 => 20.0,
        4 => 18.0,
        5 => 16.0,
        _ => 15.0,
    }
}

pub fn base_size(kind: BlockKind) -> f32 {
    match kind {
        BlockKind::Heading(level) => heading_size(level),
        BlockKind::Code | BlockKind::Table => 14.0,
        BlockKind::Image => 12.0,
        _ => 18.0,
    }
}

fn font_for(kind: BlockKind, style: Style) -> FontId {
    let size = base_size(kind);
    if style.code {
        FontId::new(size * 0.9, FontFamily::Monospace)
    } else if style.bold || matches!(kind, BlockKind::Heading(_)) {
        FontId::new(size, FontFamily::Name(BOLD_FAMILY.into()))
    } else {
        FontId::proportional(size)
    }
}

fn format_for(kind: BlockKind, style: Style, theme: &EditorTheme, muted: bool) -> egui::TextFormat {
    let font_id = font_for(kind, style);
    let mut color = if muted { theme.muted } else { theme.text };
    if style.link {
        color = theme.link;
    }
    let background = if style.code {
        theme.code_bg
    } else {
        Color32::TRANSPARENT
    };
    egui::TextFormat {
        font_id,
        color,
        background,
        italics: style.italic,
        underline: if style.link {
            Stroke::new(1.0_f32, theme.link)
        } else {
            Stroke::NONE
        },
        strikethrough: if style.strike {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

fn char_byte_offsets(text: &str) -> Vec<usize> {
    text.char_indices().map(|(i, _)| i).chain([text.len()]).collect()
}

/// Build the layout job for one block's display text.
/// `wrap_width` drives word wrap; quote lines render muted.
pub fn block_job(
    block: &Block,
    disp: &DisplayText,
    theme: &EditorTheme,
    wrap_width: f32,
) -> LayoutJob {
    let muted = block.kind == BlockKind::Quote || block.kind == BlockKind::Image;
    let mut job = LayoutJob {
        text: disp.display.clone(),
        wrap: TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        ..Default::default()
    };
    if block.kind == BlockKind::Code {
        for (byte_range, color) in code_colors(&disp.display, &block.text, theme) {
            job.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range,
                format: egui::TextFormat {
                    font_id: FontId::monospace(base_size(BlockKind::Code) - 0.5),
                    color,
                    background: Color32::TRANSPARENT,
                    italics: false,
                    underline: Stroke::NONE,
                    strikethrough: Stroke::NONE,
                    ..Default::default()
                },
            });
        }
        return job;
    }
    let offsets = char_byte_offsets(&disp.display);
    for run in &disp.runs {
        let start = offsets[run.chars.start];
        let end = offsets[run.chars.end.min(offsets.len() - 1)];
        if end > start {
            job.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: start..end,
                format: format_for(block.kind, run.style, theme, muted),
            });
        }
    }
    if job.sections.is_empty() {
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: 0..job.text.len(),
            format: format_for(block.kind, Style::default(), theme, muted),
        });
    }
    job
}

// ---------------------------------------------------------------------------
// syntect highlighting for fenced code blocks

fn syntax_sets() -> &'static (syntect::parsing::SyntaxSet, syntect::highlighting::ThemeSet) {
    static SETS: OnceLock<(syntect::parsing::SyntaxSet, syntect::highlighting::ThemeSet)> =
        OnceLock::new();
    SETS.get_or_init(|| {
        (
            syntect::parsing::SyntaxSet::load_defaults_newlines(),
            syntect::highlighting::ThemeSet::load_defaults(),
        )
    })
}

/// Colored byte ranges over the code block's display text.
fn code_colors(display: &str, raw: &str, theme: &EditorTheme) -> Vec<(std::ops::Range<usize>, Color32)> {
    use syntect::easy::HighlightLines;
    let (syntaxes, themes) = syntax_sets();
    let language = line_content(raw)
        .trim_start_matches('`')
        .trim_start_matches('~')
        .trim();
    let syntax = syntaxes
        .find_syntax_by_token(language)
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let theme_name = if theme.light {
        "base16-ocean.light"
    } else {
        "base16-ocean.dark"
    };
    let colors = themes
        .themes
        .get(theme_name)
        .map(|t| t)
        .or_else(|| themes.themes.values().next());
    let fallback = Color32::from_gray(if theme.light { 60 } else { 200 });
    let Some(syn_theme) = colors else {
        return vec![(0..display.len(), fallback)];
    };
    let mut highlighter = HighlightLines::new(syntax, syn_theme);
    let mut ranges = Vec::new();
    let mut offset = 0;
    for line in display.split_inclusive('\n') {
        match highlighter.highlight_line(line, syntaxes) {
            Ok(spans) => {
                let mut line_offset = offset;
                for (style, text) in spans {
                    let end = line_offset + text.len();
                    if end > line_offset {
                        ranges.push((
                            line_offset..end,
                            Color32::from_rgba_premultiplied(
                                style.foreground.r,
                                style.foreground.g,
                                style.foreground.b,
                                style.foreground.a,
                            ),
                        ));
                    }
                    line_offset = end;
                }
            }
            Err(_) => ranges.push((offset..offset + line.len(), fallback)),
        }
        offset += line.len();
    }
    ranges
}

// ---------------------------------------------------------------------------
// Galley helpers: cursor <-> source offsets

/// Zero-width caret rect (in galley coordinates) for a raw source offset.
pub fn caret_rect(galley: &Galley, disp: &DisplayText, off: usize) -> Rect {
    let index = disp.disp_index(off);
    galley.pos_from_ccursor(egui::text::CCursor::new(index))
}

/// Raw source offset for a click position (galley coordinates).
pub fn offset_from_pos(galley: &Galley, disp: &DisplayText, pos: Vec2) -> usize {
    let cursor = galley.cursor_from_pos(pos);
    disp.src_at_disp(cursor.ccursor.index)
}

/// Highlight rects (galley coordinates) covering `start..end` raw offsets.
pub fn selection_rects(galley: &Galley, disp: &DisplayText, start: usize, end: usize) -> Vec<Rect> {
    if start >= end {
        return Vec::new();
    }
    let from = disp.disp_index(start);
    let to = disp.disp_index(end);
    if from >= to {
        return Vec::new();
    }
    let mut rects = Vec::new();
    let mut row_start = 0;
    for row in &galley.rows {
        let row_len = row.char_count_including_newline();
        let row_end = row_start + row_len;
        let a = from.max(row_start);
        let b = to.min(row_end);
        if a < b {
            let x0 = row.x_offset(a - row_start);
            let x1 = row.x_offset((b - row_start).min(row.char_count_excluding_newline()));
            rects.push(Rect::from_x_y_ranges(x0..=x1, row.rect.y_range()));
        }
        row_start = row_end;
    }
    rects
}

/// Rows of a galley as (row index, first display char) — used by Home/End.
pub fn row_start_chars(galley: &Galley) -> Vec<usize> {
    let mut starts = Vec::with_capacity(galley.rows.len());
    let mut index = 0;
    for row in &galley.rows {
        starts.push(index);
        index += row.char_count_including_newline();
    }
    starts
}

/// Raw offset one visual row up/down from `off` inside the same block.
pub fn vertical_offset(
    galley: &Galley,
    disp: &DisplayText,
    off: usize,
    direction: i32,
    preferred_x: f32,
) -> Option<usize> {
    let cursor = galley.from_ccursor(egui::text::CCursor::new(disp.disp_index(off)));
    let target = (cursor.rcursor.row as i32).checked_add(direction)?;
    let row = galley.rows.get(usize::try_from(target).ok()?)?;
    Some(offset_from_pos(galley, disp, Vec2::new(preferred_x, row.rect.center().y)))
}
