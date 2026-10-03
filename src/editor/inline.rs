//! Inline parsing: raw block text → display text with a source map.
//!
//! The display text is what the galley shows: the hidden block prefix and the
//! markers of *matched* pairs (`**`, `*`, `` ` ``, `~~`, `[]()`, `[[]]`) are
//! omitted; unbalanced markers stay visible so nothing silently vanishes and
//! re-parsing after an edit naturally re-balances pairs.
//!
//! The source map is one `(start, end)` source range per displayed character,
//! so editing works in raw source coordinates (where the markers still live)
//! while hit-testing and caret painting work in display coordinates through
//! the egui galley.

use super::doc::{prefix_len, BlockKind};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

impl Style {
    fn merged(self, other: Style) -> Style {
        Style {
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            code: self.code || other.code,
            strike: self.strike || other.strike,
            link: self.link || other.link,
        }
    }

    fn link(self) -> Style {
        Style {
            link: true,
            ..self
        }
    }
}

/// A styled run over a `Range<char>` of the display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub chars: Range<usize>,
    pub style: Style,
}

#[derive(Clone, Debug, Default)]
pub struct DisplayText {
    pub display: String,
    /// Source byte range of each displayed character.
    pub chars: Vec<(usize, usize)>,
    /// End of the content region (excludes the trailing line terminator).
    pub content_end: usize,
    pub runs: Vec<Run>,
}

impl DisplayText {
    fn push_str(&mut self, text: &str, src_start: usize) {
        let mut offset = src_start;
        for ch in text.chars() {
            self.chars.push((offset, offset + ch.len_utf8()));
            self.display.push(ch);
            offset += ch.len_utf8();
        }
    }

    fn add_chunk(&mut self, text: &str, src_start: usize, style: Style) {
        let start = self.chars.len();
        self.push_str(text, src_start);
        if self.chars.len() > start {
            self.runs.push(Run {
                chars: start..self.chars.len(),
                style,
            });
        }
    }

    fn add_literal(&mut self, ch: char, src: Range<usize>, style: Style) {
        let start = self.chars.len();
        self.chars.push((src.start, src.end));
        self.display.push(ch);
        self.runs.push(Run {
            chars: start..self.chars.len(),
            style,
        });
    }

    fn merge_runs(&mut self) {
        let mut merged: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.style == run.style && last.chars.end == run.chars.start {
                    last.chars.end = run.chars.end;
                    continue;
                }
            }
            merged.push(run);
        }
        self.runs = merged;
    }

    /// Cursor positions where the caret can rest: the start of every displayed
    /// character plus the end of the content. Hidden marker spans are skipped.
    pub fn positions(&self) -> Vec<usize> {
        let mut positions: Vec<usize> = self.chars.iter().map(|(start, _)| *start).collect();
        positions.push(self.content_end);
        positions.dedup();
        positions
    }

    pub fn position_before(&self, off: usize) -> Option<usize> {
        self.positions()
            .into_iter()
            .filter(|p| *p < off)
            .next_back()
    }

    pub fn position_after(&self, off: usize) -> Option<usize> {
        self.positions().into_iter().find(|p| *p > off)
    }

    /// Display character index that the caret at `off` paints in front of.
    pub fn disp_index(&self, off: usize) -> usize {
        self.chars
            .iter()
            .position(|(start, _)| *start >= off)
            .unwrap_or(self.chars.len())
    }

    /// Source offset of display character index `index` (or `content_end`).
    pub fn src_at_disp(&self, index: usize) -> usize {
        self.chars
            .get(index)
            .map(|(start, _)| *start)
            .unwrap_or(self.content_end)
    }

    /// End of the first displayed character beyond `off` (for shift+Right:
    /// selections stop at visible-character boundaries, never mid-marker).
    pub fn char_end_after(&self, off: usize) -> Option<usize> {
        self.chars.iter().map(|(_, end)| *end).find(|end| *end > off)
    }

    /// End of the last displayed character before `off` (for shift+Left).
    pub fn char_end_before(&self, off: usize) -> Option<usize> {
        self.chars
            .iter()
            .map(|(_, end)| *end)
            .filter(|end| *end < off)
            .next_back()
    }

    /// The source range of one displayed character (for deletion).
    pub fn char_range(&self, index: usize) -> Option<Range<usize>> {
        self.chars.get(index).map(|(s, e)| *s..*e)
    }

    /// Source range of the word around `off` (double-click selection).
    pub fn word_range(&self, off: usize) -> Range<usize> {
        let display: Vec<char> = self.display.chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut start = self.disp_index(off);
        while start > 0 && is_word(display[start - 1]) {
            start -= 1;
        }
        let mut end = start;
        while end < display.len() && is_word(display[end]) {
            end += 1;
        }
        let from = self.src_at_disp(start);
        let to = if end > 0 {
            self.char_range(end - 1).map(|r| r.end).unwrap_or(from)
        } else {
            from
        };
        from..to.max(from)
    }

    /// Next word boundary after `off` (Ctrl+Right).
    pub fn next_word_position(&self, off: usize) -> usize {
        let display: Vec<char> = self.display.chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut i = self.disp_index(off);
        while i < display.len() && is_word(display[i]) {
            i += 1;
        }
        while i < display.len() && !is_word(display[i]) {
            i += 1;
        }
        self.src_at_disp(i)
    }

    /// Previous word boundary before `off` (Ctrl+Left).
    pub fn prev_word_position(&self, off: usize) -> usize {
        let display: Vec<char> = self.display.chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut i = self.disp_index(off);
        while i > 0 && !is_word(display[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(display[i - 1]) {
            i -= 1;
        }
        self.src_at_disp(i)
    }
}

/// Parse one block into display text with source mapping and style runs.
pub fn parse(text: &str, kind: BlockKind) -> DisplayText {
    let mut out = DisplayText {
        content_end: text.len(),
        ..Default::default()
    };
    let prefix = prefix_len(text, kind);
    let terminator = if text.ends_with("\r\n") {
        2
    } else if text.ends_with('\n') {
        1
    } else {
        0
    };
    out.content_end = text.len() - terminator;
    // The block prefix (`# `, `- `, `> `) is hidden entirely.
    let content = &text[prefix..out.content_end];
    if matches!(
        kind,
        BlockKind::Code | BlockKind::Table | BlockKind::Image | BlockKind::Rule
    ) {
        // Code/tables/images edit their raw text: no marker hiding at all, the
        // map is the identity (except `\r\n`, one displayed newline per pair).
        let mut i = 0;
        while i < content.len() {
            let ch = content[i..].chars().next().unwrap_or('\u{fffd}');
            if ch == '\r' && content[i..].starts_with("\r\n") {
                out.add_literal('\n', (prefix + i)..(prefix + i + 2), Style::default());
                i += 2;
            } else if ch == '\r' {
                out.add_literal('\n', (prefix + i)..(prefix + i + 1), Style::default());
                i += 1;
            } else {
                out.add_literal(
                    ch,
                    (prefix + i)..(prefix + i + ch.len_utf8()),
                    Style::default(),
                );
                i += ch.len_utf8();
            }
        }
        return out;
    }
    let mut scanner = Scanner {
        text: content,
        base: prefix,
        out: &mut out,
    };
    scanner.run(Style::default());
    out.merge_runs();
    out
}

struct Scanner<'a> {
    text: &'a str,
    base: usize,
    out: &'a mut DisplayText,
}

/// A special (non-plain) stretch of source text.
enum Segment<'a> {
    /// Shown character with an explicit source range (escapes, `\r\n`).
    Literal(char, Range<usize>),
    /// A matched pair: the style applies to its content, markers are hidden.
    Nested(Style, Range<usize>),
    /// A link label; the `[text](url)` / `[[target|label]]` wrapper is hidden.
    Link(&'a str, Range<usize>),
}

impl<'a> Scanner<'a> {
    fn run(&mut self, style: Style) {
        let text = self.text;
        let base = self.base;
        let mut plain_start = 0;
        let mut i = 0;
        while i < text.len() {
            let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
            match detect(text, i, ch) {
                None => i += ch.len_utf8(),
                Some((segment, len)) => {
                    if i > plain_start {
                        self.out
                            .add_chunk(&text[plain_start..i], base + plain_start, style);
                    }
                    match segment {
                        Segment::Literal(c, range) => {
                            self.out
                                .add_literal(c, (base + range.start)..(base + range.end), style);
                        }
                        Segment::Nested(inner_style, inner_range) => {
                            let mut inner = Scanner {
                                text: &text[inner_range.clone()],
                                base: base + inner_range.start,
                                out: &mut *self.out,
                            };
                            inner.run(style.merged(inner_style));
                        }
                        Segment::Link(label, label_range) => {
                            self.out
                                .add_chunk(label, base + label_range.start, style.link());
                        }
                    }
                    i += len;
                    plain_start = i;
                }
            }
        }
        if plain_start < text.len() {
            self.out
                .add_chunk(&text[plain_start..], base + plain_start, style);
        }
    }
}

fn detect(text: &str, at: usize, ch: char) -> Option<(Segment<'_>, usize)> {
    let rest = &text[at..];
    match ch {
        '\\' if at + 1 < text.len() => {
            let next = text[at + 1..].chars().next()?;
            Some((
                Segment::Literal(next, (at + 1)..(at + 1 + next.len_utf8())),
                1 + next.len_utf8(),
            ))
        }
        // `\r\n` renders (and deletes) as one newline covering both bytes.
        '\r' if rest.starts_with("\r\n") => Some((Segment::Literal('\n', at..at + 2), 2)),
        '\r' => Some((Segment::Literal('\n', at..at + 1), 1)),
        '*' | '_' => {
            let (marker, bold) = if rest.starts_with("**") {
                ("**", true)
            } else if rest.starts_with("__") {
                ("__", true)
            } else {
                (if ch == '*' { "*" } else { "_" }, false)
            };
            if !boundary_ok(text, at, ch) {
                return None;
            }
            let end = find_close(text, at + marker.len(), marker)?;
            let style = if bold {
                Style {
                    bold: true,
                    ..Default::default()
                }
            } else {
                Style {
                    italic: true,
                    ..Default::default()
                }
            };
            Some((
                Segment::Nested(style, (at + marker.len())..end),
                end + marker.len() - at,
            ))
        }
        '~' if rest.starts_with("~~") => {
            let end = find_close(text, at + 2, "~~")?;
            Some((
                Segment::Nested(
                    Style {
                        strike: true,
                        ..Default::default()
                    },
                    (at + 2)..end,
                ),
                end + 2 - at,
            ))
        }
        '`' => {
            let end = find_close(text, at + 1, "`")?;
            Some((
                Segment::Nested(
                    Style {
                        code: true,
                        ..Default::default()
                    },
                    (at + 1)..end,
                ),
                end + 1 - at,
            ))
        }
        '[' | '!' => link_segment(text, at),
        _ => None,
    }
}

/// `_` only opens italics at word boundaries (`snake_case` stays literal).
fn boundary_ok(text: &str, at: usize, marker: char) -> bool {
    if marker != '_' {
        return true;
    }
    let before = text[..at].chars().next_back();
    let after = text[at + 1..].chars().next();
    !matches!(before, Some(c) if c.is_alphanumeric() || c == '_')
        && !matches!(after, Some(c) if c.is_alphanumeric() || c == '_')
}

/// Index just past the matching `marker` at or after `from`, if any.
fn find_close(text: &str, from: usize, marker: &str) -> Option<usize> {
    let rest = &text[from..];
    let mut offset = 0;
    while offset < rest.len() {
        if rest[offset..].starts_with(marker) {
            return Some(from + offset);
        }
        offset += rest[offset..].chars().next()?.len_utf8();
    }
    None
}

/// `[label](url)`, `![alt](path)`, `[[target]]`, `[[target|alias]]`, `![[image]]`.
fn link_segment(text: &str, at: usize) -> Option<(Segment<'_>, usize)> {
    let rest = &text[at..];
    if !(rest.starts_with('[') || rest.starts_with("![")) {
        return None;
    }
    let open = if rest.starts_with("![") { at + 1 } else { at };
    // Wiki links: [[target]] / [[target|alias]] / ![[image]]
    if text[open..].starts_with("[[") {
        let close = text[open + 2..].find("]]")? + open + 2;
        let inner = &text[open + 2..close];
        if inner.contains("[[") {
            return None;
        }
        let (label, label_start) = match inner.split_once('|') {
            Some((_, alias)) => (alias, close - alias.len()),
            None => (inner, open + 2),
        };
        return Some((Segment::Link(label, label_start..close), close + 2 - at));
    }
    if !text[open..].starts_with('[') {
        return None;
    }
    let bracket = text[open + 1..].find(']')? + open + 1;
    if !text[bracket + 1..].starts_with('(') {
        return None;
    }
    let paren = text[bracket + 2..].find(')')? + bracket + 2;
    let label = &text[open + 1..bracket];
    Some((Segment::Link(label, (open + 1)..bracket), paren + 1 - at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> DisplayText {
        parse(text, BlockKind::Paragraph)
    }

    #[test]
    fn matched_markers_hide_and_map_round_trips() {
        let source = "a**x**b";
        let disp = plain(source);
        assert_eq!(disp.display, "axb");
        // Every displayed character maps back to its own source byte.
        assert_eq!(disp.chars, vec![(0, 1), (3, 4), (6, 7)]);
        for (index, (start, end)) in disp.chars.iter().enumerate() {
            assert_eq!(disp.disp_index(*start), index);
            assert_eq!(disp.src_at_disp(index), *start);
            assert_eq!(source[*start..*end].chars().count(), 1);
        }
        assert_eq!(disp.content_end, source.len());
    }

    #[test]
    fn unbalanced_markers_stay_visible() {
        let disp = plain("a**b");
        assert_eq!(disp.display, "a**b");
        assert_eq!(disp.chars.len(), 4);
    }

    #[test]
    fn styles_are_detected() {
        let disp = plain("**g** *c* `k` ~~s~~ [l](u) [[w|alias]]");
        assert_eq!(disp.display, "g c k s l alias");
        let styled = |index: usize| {
            disp.runs
                .iter()
                .find(|r| r.chars.contains(&index))
                .unwrap()
                .style
        };
        assert!(styled(0).bold);
        assert!(styled(2).italic);
        assert!(styled(4).code);
        assert!(styled(6).strike);
        assert!(styled(8).link);
        assert!(styled(10).link);
    }

    #[test]
    fn block_prefix_is_hidden() {
        let disp = parse("# Titolo\n", BlockKind::Heading(1));
        assert_eq!(disp.display, "Titolo");
        assert_eq!(disp.chars[0], (2, 3));
        assert_eq!(disp.content_end, 8);

        let disp = parse(
            "- [x] voce\n",
            BlockKind::ListItem {
                ordered: false,
                task: true,
            },
        );
        assert_eq!(disp.display, "voce");
    }

    #[test]
    fn trailing_terminator_is_out_of_the_map() {
        let disp = plain("abc\r\ndef\r\n");
        // The internal \r\n collapses to one displayed newline covering both
        // bytes; the trailing terminator is outside the content region.
        assert_eq!(disp.display, "abc\ndef");
        assert_eq!(disp.chars[3], (3, 5));
        assert_eq!(disp.content_end, 8);
        assert_eq!(disp.positions(), vec![0, 1, 2, 3, 5, 6, 7, 8]);
    }

    #[test]
    fn caret_navigation_skips_hidden_markers() {
        let disp = plain("a**x**b");
        // From just before `b` the caret jumps over the closing `**`.
        assert_eq!(disp.position_before(6), Some(3));
        assert_eq!(disp.position_after(3), Some(6));
        // Typing positions hug the markers' outer edges.
        assert_eq!(disp.position_after(0), Some(3));
    }

    #[test]
    fn deleting_a_char_keeps_only_its_own_bytes() {
        let disp = plain("a**x**b");
        assert_eq!(disp.char_range(1), Some(3..4));
    }

    #[test]
    fn word_ranges_and_jumps() {
        let disp = plain("ciao **mondo** fine");
        assert_eq!(disp.display, "ciao mondo fine");
        assert_eq!(disp.word_range(6), 7..12);
        assert_eq!(disp.next_word_position(0), 7);
        assert_eq!(disp.prev_word_position(15), 7);
    }

    #[test]
    fn escaped_markers_are_literal() {
        let disp = plain(r"\*nota\*");
        assert_eq!(disp.display, "*nota*");
    }

    #[test]
    fn snake_case_is_not_italic() {
        let disp = plain("nome_variabile");
        assert_eq!(disp.display, "nome_variabile");
    }
}
